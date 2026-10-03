//! End-to-end: a helper stream becomes a searchable index on the client side.
//!
//! This runs the same `client::read_index` the app calls, so it covers the
//! framing, the request handshake and the `FileDbBuilder` materialisation in one
//! go. A walk root is used so no admin is required.
#![cfg(windows)]

use std::time::Duration;

use steward_core_engine::file_index::{search, IndexBackend, SearchOptions};
use steward_index_helper::client::{self, Client};
use steward_index_helper::server;
use steward_index_helper::StreamRequest;

#[test]
fn a_streamed_walk_builds_a_searchable_index() {
    let root = std::env::temp_dir().join(format!(
        "steward-helper-build-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("top.txt"), b"top").unwrap();
    std::fs::write(root.join("sub").join("deep.txt"), b"deep").unwrap();

    let pipe = format!(
        r"\\.\pipe\steward-index-build-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let server_pipe = pipe.clone();
    let server_thread = std::thread::spawn(move || server::serve_once(&server_pipe));

    let mut client = None;
    for _ in 0..100 {
        match Client::connect(&pipe) {
            Ok(connected) => {
                client = Some(connected);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut client = client.expect("the helper pipe must come up");
    client
        .send_request(&StreamRequest::new([root.to_string_lossy().into_owned()]))
        .unwrap();
    let (index, backends, _truncated) = client::read_index(&mut client).unwrap();

    assert_eq!(backends.len(), 1);
    assert_eq!(backends[0].1, IndexBackend::Walk);

    let outcome = search(&index, "deep", &SearchOptions::with_limit(10));
    assert_eq!(outcome.hits.len(), 1, "deep.txt must be searchable");
    assert_eq!(outcome.hits[0].name, "deep.txt");
    assert!(outcome.hits[0]
        .path
        .to_string_lossy()
        .ends_with(&format!("sub{}deep.txt", std::path::MAIN_SEPARATOR)));

    let served = server_thread.join().unwrap().unwrap();
    assert_eq!(served.records as usize, index.slot_count());
    std::fs::remove_dir_all(&root).unwrap();
}
