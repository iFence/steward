//! 3-gram inverted index over record names.
//!
//! A linear scan over millions of records costs hundreds of milliseconds per
//! keystroke, which is why the plan gates this accelerator on a record count.
//! Queries whose needle is a folded substring of at least three bytes are
//! answered from postings instead.
//!
//! Layout: a sorted term table (`trigram -> offset/len` into a postings blob)
//! plus one serialized [`RoaringBitmap`] per trigram. The table is a plain
//! sorted `Vec` (binary search), and only the few bitmaps a needle names are
//! ever decoded.
//!
//! Correctness contract: postings may *over*-include (a deleted or renamed
//! record stays in an old bitmap) because every candidate is verified against
//! its current name. They must never under-include, so names added after the
//! build go into an overflow map.

use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::db::FileDb;

/// Below this many slots the linear scan is fast enough and the extra memory is
/// not worth it.
pub const MIN_RECORDS: usize = 100_000;

/// ASCII-fold one byte (the default search case mode).
fn fold(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte + (b'a' - b'A')
    } else {
        byte
    }
}

/// Pack a folded 3-byte window into a sortable key.
fn key(window: &[u8]) -> u32 {
    ((window[0] as u32) << 16) | ((window[1] as u32) << 8) | window[2] as u32
}

/// The distinct folded trigrams of `bytes`, appended to `out`.
pub(crate) fn trigrams_into(bytes: &[u8], out: &mut Vec<u32>) {
    out.clear();
    if bytes.len() < 3 {
        return;
    }
    let mut folded = Vec::with_capacity(3);
    for window in bytes.windows(3) {
        folded.clear();
        folded.extend(window.iter().map(|byte| fold(*byte)));
        let key = key(&folded);
        if !out.contains(&key) {
            out.push(key);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Term {
    key: u32,
    offset: u32,
    len: u32,
}

/// The 3-gram accelerator. Derived state: never persisted.
pub struct NameIndex {
    terms: Vec<Term>,
    postings: Vec<u8>,
    /// Names added after the build, keyed by trigram. Small because a full
    /// rebuild happens on every launch (the helper re-enumerates).
    overflow: HashMap<u32, Vec<u32>>,
    /// Slots covered by `terms`/`postings`; later slots were folded into the
    /// overflow.
    built_slots: u32,
    records: usize,
}

impl NameIndex {
    /// Build the accelerator, or `None` when the index is too small.
    pub fn build(index: &FileDb) -> Option<Self> {
        Self::build_with_threshold(index, MIN_RECORDS)
    }

    pub(crate) fn build_with_threshold(index: &FileDb, min_records: usize) -> Option<Self> {
        if index.slot_count() < min_records {
            return None;
        }
        let mut bitmaps: HashMap<u32, RoaringBitmap> = HashMap::new();
        let mut scratch = Vec::new();
        for record in 0..index.slot_count() as u32 {
            if !index.is_live(record) {
                continue;
            }
            trigrams_into(index.name_bytes(record), &mut scratch);
            for key in &scratch {
                bitmaps.entry(*key).or_default().insert(record);
            }
        }

        let mut entries: Vec<(u32, RoaringBitmap)> = bitmaps.into_iter().collect();
        entries.sort_unstable_by_key(|(key, _)| *key);
        let mut postings = Vec::new();
        let mut terms = Vec::with_capacity(entries.len());
        for (key, bitmap) in entries {
            let offset = postings.len() as u32;
            bitmap.serialize_into(&mut postings).ok()?;
            terms.push(Term {
                key,
                offset,
                len: postings.len() as u32 - offset,
            });
        }
        Some(Self {
            terms,
            postings,
            overflow: HashMap::new(),
            built_slots: index.slot_count() as u32,
            records: index.len(),
        })
    }

    /// How many live records the build covered.
    pub fn records(&self) -> usize {
        self.records
    }

    pub fn is_empty(&self) -> bool {
        self.records == 0
    }

    /// Bytes held by the accelerator (postings + table + overflow keys).
    pub fn approx_bytes(&self) -> usize {
        self.postings.len()
            + self.terms.len() * std::mem::size_of::<Term>()
            + self.overflow.len() * 24
    }

    /// Slot count covered by the postings blob.
    pub(crate) fn built_slots(&self) -> u32 {
        self.built_slots
    }

    /// Fold a newly appended record's name into the overflow.
    pub(crate) fn note(&mut self, record: u32, name: &[u8]) {
        let mut scratch = Vec::new();
        trigrams_into(name, &mut scratch);
        for key in scratch {
            let entry = self.overflow.entry(key).or_default();
            if !entry.contains(&record) {
                entry.push(record);
            }
        }
        self.records += 1;
        self.built_slots = self.built_slots.max(record + 1);
    }

    fn lookup(&self, key: u32) -> Option<&Term> {
        self.terms
            .binary_search_by_key(&key, |term| term.key)
            .ok()
            .map(|position| &self.terms[position])
    }

    /// Candidate record indices for a folded `needle`, or `None` when the
    /// needle is shorter than a trigram (the caller falls back to the scan).
    pub fn candidates(&self, needle: &[u8]) -> Option<Vec<u32>> {
        if needle.len() < 3 {
            return None;
        }
        // Fold the needle once; `trigrams_into` folds internally, so feed it the
        // raw bytes.
        let mut keys = Vec::new();
        trigrams_into(needle, &mut keys);

        let mut terms: Vec<Term> = Vec::with_capacity(keys.len());
        for key in &keys {
            match self.lookup(*key) {
                Some(term) => terms.push(*term),
                // A trigram of the needle is absent, so no name can contain it.
                None => return Some(Vec::new()),
            }
        }
        // Rarest first keeps the intersection small early.
        terms.sort_unstable_by_key(|term| term.len);

        let mut result: Option<RoaringBitmap> = None;
        for term in terms {
            let bytes = &self.postings[term.offset as usize..(term.offset + term.len) as usize];
            let bitmap = RoaringBitmap::deserialize_from(bytes).ok()?;
            result = Some(match result {
                Some(accumulated) => accumulated & bitmap,
                None => bitmap,
            });
            if result.as_ref().is_some_and(RoaringBitmap::is_empty) {
                break;
            }
        }

        let mut candidates: Vec<u32> = result
            .map(|bitmap| bitmap.into_iter().collect())
            .unwrap_or_default();
        for key in &keys {
            if let Some(records) = self.overflow.get(key) {
                candidates.extend_from_slice(records);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        Some(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::EntryInfo;
    use crate::file_index::{search, FileDbBuilder, SearchOptions};

    fn build(names: &[&str]) -> (FileDb, NameIndex) {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        for name in names {
            builder.add_child(root, &EntryInfo::file(*name));
        }
        let index = builder.finalize();
        // Bypass the size gate for the unit test.
        let accelerator =
            NameIndex::build_with_threshold(&index, 1).expect("a threshold of 1 always builds");
        (index, accelerator)
    }

    #[test]
    fn trigrams_fold_ascii_and_deduplicate() {
        let mut keys = Vec::new();
        trigrams_into(b"Report", &mut keys);
        let mut expected = Vec::new();
        trigrams_into(b"report", &mut expected);
        assert_eq!(keys, expected);
        assert_eq!(keys.len(), 4, "rep epo por ort");
    }

    #[test]
    fn candidates_are_a_superset_of_the_scan() {
        let (index, accelerator) = build(&[
            "annual-report.pdf",
            "report.pdf",
            "myreport.txt",
            "notes.md",
        ]);
        for needle in ["rep", "report", "port", "annual", "notes"] {
            let candidates = accelerator.candidates(needle.as_bytes()).unwrap();
            let expected: Vec<u32> = search(&index, needle, &SearchOptions::with_limit(50))
                .hits
                .iter()
                .map(|hit| hit.index)
                .collect();
            for record in expected {
                assert!(
                    candidates.contains(&record),
                    "{needle}: {record} must be a candidate"
                );
            }
        }
    }

    #[test]
    fn a_missing_trigram_yields_no_candidates() {
        let (_index, accelerator) = build(&["report.pdf", "notes.md"]);
        assert!(accelerator.candidates(b"zzz").unwrap().is_empty());
        // Short needles defer to the scan.
        assert!(accelerator.candidates(b"re").is_none());
    }

    #[test]
    fn names_added_after_the_build_land_in_the_overflow() {
        let (mut index, mut accelerator) = build(&["report.pdf"]);
        let root = index
            .iter_ordered()
            .find(|record| index.parent_of(*record) == crate::file_index::ROOT_PARENT)
            .expect("the root record");
        let added = index
            .insert_child(root, &EntryInfo::file("new-report.txt"))
            .unwrap();
        index.finish_incremental();
        accelerator.note(added, index.name_bytes(added));
        let candidates = accelerator.candidates(b"report").unwrap();
        assert!(candidates.contains(&added));
    }
}
