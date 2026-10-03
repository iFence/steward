//! Manual scale harness for the resident file index.
//!
//! The plan tracks file-index numbers separately from the app-index ones in
//! `docs/benchmarks.md`, but a real full-volume run needs an elevated `$MFT`
//! read and a quiet machine. This test builds a synthetic index of the same
//! shape (one-level tree, `STEWARD_BENCH_RECORDS` records, default 300k) and
//! reports build time, the dominant memory figure, per-needle query latency and
//! incremental-batch cost, so regressions in the hot path are measurable:
//!
//! ```text
//! cargo test -p steward-core-engine --release --test file_index_bench -- --ignored --nocapture
//! STEWARD_BENCH_RECORDS=3000000 cargo test -p steward-core-engine --release \
//!     --test file_index_bench -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use steward_core_engine::file_index::{self, EntryInfo, FileDbBuilder, KindFilter, SearchOptions};

#[test]
#[ignore = "manual performance harness"]
fn file_index_scale_report() {
    let records: usize = std::env::var("STEWARD_BENCH_RECORDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300_000);
    let directories = 1024usize;

    let mut builder = FileDbBuilder::with_capacity(records + directories + 1);
    let root = builder.add_root("C:\\", 5);
    let mut folders = Vec::with_capacity(directories);
    for dir in 0..directories {
        folders.push(builder.add_child(root, &EntryInfo::dir(format!("dir{dir:04}"))));
    }
    for file in 0..records {
        let folder = folders[file % directories];
        builder.add_child(
            folder,
            &EntryInfo::file(format!("report-{file:07}.txt"))
                .with_size(file as u64)
                .with_mtime(1_700_000_000 + file as u64),
        );
    }

    let started = Instant::now();
    let mut index = builder.finalize();
    let build = started.elapsed();
    println!(
        "records={} arena_bytes={} build={build:?}",
        index.len(),
        index.arena_bytes()
    );

    fn measure(label: &str, index: &steward_core_engine::file_index::FileDb) {
        let options = SearchOptions {
            limit: 12,
            kind: KindFilter::Any,
            sort_by_recency: false,
        };
        for needle in ["a", "re", "rep", "report", "report-00012"] {
            let mut samples: Vec<Duration> = Vec::with_capacity(20);
            for _ in 0..20 {
                let started = Instant::now();
                let outcome = file_index::search_filtered(
                    index,
                    &file_index::parse_filter(needle),
                    &options,
                    None,
                );
                std::hint::black_box(outcome.hits.len());
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            println!(
                "{label} needle={needle:?} p50={:?} p95={:?}",
                samples[samples.len() / 2],
                samples[(samples.len() * 95) / 100]
            );
        }
    }

    measure("scan   ", &index);
    let started = Instant::now();
    let built = index.build_name_index();
    println!(
        "name_index built={built} bytes={} build={:?}",
        index.name_index().map_or(0, |accel| accel.approx_bytes()),
        started.elapsed()
    );
    if built {
        measure("3-gram ", &index);
    }

    // One live batch: insert `batch` records, remove half of them, fold the
    // batch in once. This is the cost a busy disk pays per watcher/ USN batch.
    let batch = 1000usize.min(records.max(1));
    let mut inserted = Vec::with_capacity(batch);
    let started = Instant::now();
    for file in 0..batch {
        let folder = folders[file % directories];
        if let Some(index) =
            index.insert_child(folder, &EntryInfo::file(format!("bench-{file:07}.txt")))
        {
            inserted.push(index);
        }
    }
    for record in inserted.iter().take(inserted.len() / 2).copied() {
        index.remove_subtree(record);
    }
    index.finish_incremental();
    println!(
        "incremental_batch={batch} apply={:?} records={}",
        started.elapsed(),
        index.len()
    );
}
