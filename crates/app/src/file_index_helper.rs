//! Build the file index from the optional privileged helper.
//!
//! The helper (see `crates/index-helper`) runs with enough privilege to read a
//! volume's `$MFT` and streams the records over a named pipe. This module
//! consumes that stream into the same `FileDbBuilder` the local walk uses, so
//! the rest of the index pipeline does not care which enumerator ran.
//!
//! The helper is strictly an accelerator: `try_build` returns `None` when it is
//! not running, and the caller falls back to the in-process directory walk.

use crate::file_index::BuildOutput;
use steward_core_engine::file_index::ScanOptions;

/// A live helper connection: the snapshot is built, the stream stays open for
/// USN deltas.
pub(crate) struct HelperSession {
    #[cfg(target_os = "windows")]
    pub(crate) client: steward_index_helper::client::Client,
}

#[cfg(target_os = "windows")]
impl HelperSession {
    /// Read frames on a dedicated thread and hand them to the index worker.
    ///
    /// The thread exits when the helper closes the pipe or the worker drops the
    /// receiver; it never touches the index itself, so the worker stays the only
    /// writer.
    pub(crate) fn start(
        self,
    ) -> crossbeam_channel::Receiver<steward_index_helper::protocol::Frame> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut client = self.client;
        let _ = std::thread::Builder::new()
            .name("steward-helper-deltas".into())
            .spawn(move || {
                // Stops on a closed pipe, a protocol error, or a dropped
                // receiver (the worker replaced this session).
                while let Ok(Some(frame)) = client.read_frame() {
                    if sender.send(frame).is_err() {
                        break;
                    }
                }
            });
        receiver
    }
}

/// Try to build from the helper.
///
/// `None` means "no helper is listening, use the local walk". `Some(Err(..))`
/// means the helper answered but the stream failed, which is worth logging
/// before falling back.
#[cfg(target_os = "windows")]
pub(crate) fn try_build(
    options: &ScanOptions,
) -> Option<Result<(BuildOutput, Option<HelperSession>), String>> {
    use steward_index_helper::client::{self, Client};
    use steward_index_helper::protocol::{PIPE_NAME, PROTOCOL_VERSION};
    use steward_index_helper::StreamRequest;

    let mut client = match Client::connect(PIPE_NAME) {
        Ok(client) => client,
        Err(_) => return None,
    };
    let request = StreamRequest {
        version: PROTOCOL_VERSION,
        roots: options
            .roots
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect(),
        excluded_dirs: options.excluded_dirs.iter().cloned().collect(),
        live: true,
    };
    if let Err(error) = client.send_request(&request) {
        return Some(Err(error.to_string()));
    }
    match client::read_index(&mut client) {
        Ok(output) => Some(Ok((output, Some(HelperSession { client })))),
        Err(error) => Some(Err(error)),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn try_build(
    _options: &ScanOptions,
) -> Option<Result<(BuildOutput, Option<HelperSession>), String>> {
    None
}
