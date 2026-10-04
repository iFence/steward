//! Synchronous named-pipe client.
//!
//! Windows lets `std::fs` open a named pipe like a file, so the client needs no
//! extra transport crate: one handle for the request line, a cloned handle
//! behind a buffered reader for the frame stream.

use std::fs::OpenOptions;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;

use steward_core_engine::file_index::{EntryInfo, FileDb, FileDbBuilder, IndexBackend};

use crate::protocol::{self, Backend, Frame, ProtocolError};
use crate::source::StreamRequest;

/// A stream materialised into an index, the backends that produced it and the
/// number of names the helper truncated.
pub type BuiltIndex = (FileDb, Vec<(PathBuf, IndexBackend)>, usize);

/// A connection to the helper, ready to send one request and read its stream.
pub struct Client {
    writer: std::fs::File,
    reader: BufReader<std::fs::File>,
}

impl Client {
    /// Connect to `pipe_name` (e.g. [`crate::protocol::PIPE_NAME`]).
    pub fn connect(pipe_name: &str) -> io::Result<Self> {
        let writer = OpenOptions::new().read(true).write(true).open(pipe_name)?;
        let reader = BufReader::with_capacity(256 * 1024, writer.try_clone()?);
        Ok(Self { writer, reader })
    }

    /// Send the request line; the helper starts streaming after this.
    pub fn send_request(&mut self, request: &StreamRequest) -> io::Result<()> {
        let mut line = serde_json::to_string(request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push('\n');
        // `File::flush` maps to `FlushFileBuffers`, which on a pipe blocks until
        // the peer has consumed everything; the write itself is already visible
        // to the reader, so do not flush here.
        self.writer.write_all(line.as_bytes())
    }

    /// Read the next frame; `Ok(None)` means the helper closed the stream.
    pub fn read_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        protocol::read_frame(&mut self.reader)
    }
}

/// Consume a stream into a fresh index: `(index, backends, truncated names)`.
///
/// `records_hint` is the client's expectation of the stream's size (the count
/// of the index being replaced); it only pre-sizes the builder so a
/// multi-million-record arena is not grown by repeated doubling.
///
/// This is the client half the app runs; keeping it here means the pipe test can
/// cover the exact materialisation path instead of only the framing.
pub fn read_index(client: &mut Client, records_hint: Option<u64>) -> Result<BuiltIndex, String> {
    let mut builder = match records_hint.and_then(|hint| usize::try_from(hint).ok()) {
        Some(expected) if expected > 0 => FileDbBuilder::with_capacity(expected),
        _ => FileDbBuilder::new(),
    };
    let mut backends: Vec<(PathBuf, IndexBackend)> = Vec::new();
    loop {
        match client.read_frame() {
            Ok(Some(Frame::Volume(volume))) => {
                let root = if volume.drive != 0 {
                    PathBuf::from(format!("{}:\\", volume.drive as char))
                } else {
                    PathBuf::from(&volume.root_name)
                };
                backends.push((
                    root,
                    match volume.backend {
                        Backend::Mft => IndexBackend::Mft,
                        Backend::Walk => IndexBackend::Walk,
                    },
                ));
            }
            Ok(Some(Frame::Record(record))) => {
                if record.is_root {
                    builder.add_root(&record.name, record.id);
                } else {
                    builder.add_entry(&EntryInfo {
                        id: record.id,
                        parent_id: record.parent_id,
                        size: record.size,
                        mtime: record.mtime,
                        attributes: record.attributes,
                        name: record.name,
                        is_dir: record.is_dir,
                    });
                }
            }
            Ok(Some(Frame::End(_))) => break,
            // Snapshot-only consumption: a delta before the snapshot ended is
            // impossible with this helper, and a resync means the snapshot is
            // unusable.
            Ok(Some(Frame::Delta(_))) => {}
            Ok(Some(Frame::Resync)) => return Err("helper requested a resync".into()),
            Ok(None) => return Err("helper closed the stream early".into()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let truncated = builder.truncated_names();
    let index = builder.finalize();
    if index.is_empty() {
        return Err("helper stream produced an empty index".into());
    }
    Ok((index, backends, truncated))
}
