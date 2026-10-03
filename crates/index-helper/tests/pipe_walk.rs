//! End-to-end check of the helper protocol over a real named pipe.
//!
//! Uses a walk root (a temp directory) so the test needs no admin: the framing,
//! the request/stream handshake and the parent linkage are exercised exactly as
//! the app exercises them.
#![cfg(windows)]

use std::collections::HashMap;
use std::time::Duration;

use steward_index_helper::client::Client;
use steward_index_helper::protocol::Frame;
use steward_index_helper::server;
use steward_index_helper::StreamRequest;

#[test]
fn a_walk_stream_crosses_a_named_pipe() {
    let root = std::env::temp_dir().join(format!(
        "steward-helper-pipe-{}-{}",
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
        r"\\.\pipe\steward-index-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let server_pipe = pipe.clone();
    let server_thread = std::thread::spawn(move || {
        let result = server::serve_once(&server_pipe);
        if let Err(error) = &result {
            eprintln!("serve_once failed: {error}");
        }
        result
    });

    // The server creates the pipe on its own thread; retry until it is up.
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

    let mut roots = 0;
    let mut records = 0;
    let mut seen: HashMap<u64, String> = HashMap::new();
    let summary = loop {
        let frame = match client.read_frame().unwrap() {
            Some(frame) => frame,
            None => {
                let served = server_thread.join().unwrap();
                panic!("helper closed before End: {served:?}");
            }
        };
        match frame {
            Frame::Volume(volume) => {
                assert_eq!(volume.backend, steward_index_helper::Backend::Walk);
            }
            Frame::Record(record) => {
                if record.is_root {
                    roots += 1;
                    assert_eq!(record.parent_id, 0);
                } else {
                    assert!(
                        seen.contains_key(&record.parent_id),
                        "parent {} must precede {}",
                        record.parent_id,
                        record.name
                    );
                }
                seen.insert(record.id, record.name);
                records += 1;
            }
            Frame::End(end) => break end,
            Frame::Delta(_) | Frame::Resync => {}
        }
    };
    assert_eq!(roots, 1);
    assert_eq!(summary.records, records);
    assert!(seen.values().any(|name| name == "deep.txt"));

    let served = server_thread.join().unwrap().unwrap();
    assert_eq!(served.records, summary.records);
    std::fs::remove_dir_all(&root).unwrap();
}
