//! Windows named-pipe server for the helper.
//!
//! One connection carries one request: a JSON line naming the roots, followed
//! by a binary frame stream. The server never holds the client's index 鈥?it
//! enumerates and streams, so its own footprint stays flat regardless of how
//! many entries the volume has.

use std::io::{self, Read, Write};

use crate::source::{self, StreamRequest, Summary};

/// Serve connections forever, one at a time.
#[cfg(target_os = "windows")]
pub fn serve(pipe_name: &str) -> io::Result<()> {
    loop {
        match serve_once(pipe_name) {
            Ok(_) => {}
            // A client that connects and disappears must not take the helper
            // down; only errors creating the pipe itself are fatal.
            Err(error) => eprintln!("index helper: connection failed: {error}"),
        }
    }
}

/// Serve exactly one connection and return what it streamed.
#[cfg(target_os = "windows")]
pub fn serve_once(pipe_name: &str) -> io::Result<Summary> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};

    let handle = create_pipe(pipe_name)?;
    let mut stream = PipeStream::new(handle);
    // A client may connect between `CreateNamedPipeW` and `ConnectNamedPipe`;
    // that is reported as `ERROR_PIPE_CONNECTED`, which is success here.
    let connected = unsafe { ConnectNamedPipe(stream.handle, std::ptr::null_mut()) };
    if connected == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_PIPE_CONNECTED {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }

    let request_line = read_request_line(&mut stream)?;
    let request: StreamRequest = serde_json::from_slice(&request_line)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let (summary, mut sessions) = source::stream_with_sessions(&request.options(), &mut stream)?;
    // A `$MFT` volume with a usable journal keeps the connection open and pumps
    // live deltas; walk-only roots close after the snapshot.
    if request.live && !sessions.is_empty() {
        pump_deltas(&mut stream, &mut sessions)?;
    }
    // `FlushFileBuffers` on a pipe waits until the client has read every byte;
    // without it a fast `DisconnectNamedPipe` discards frames the client has
    // not drained yet (ERROR_PIPE_NOT_CONNECTED on the client side).
    use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
    let _ = unsafe { FlushFileBuffers(stream.handle) };
    unsafe {
        DisconnectNamedPipe(stream.handle);
    }
    Ok(summary)
}

/// Pump live USN deltas until the client disconnects or a journal must resync.
///
/// A volume whose journal became unusable (deleted, recreated, wrapped) cannot
/// be caught up incrementally, so the client is told to rebuild.
#[cfg(target_os = "windows")]
fn pump_deltas(
    writer: &mut PipeStream,
    sessions: &mut [crate::source::VolumeSession],
) -> io::Result<()> {
    use std::time::Duration;

    use steward_core_engine::file_index::{cursor_is_usable, JournalState};

    use crate::delta::{delta_for, ChangeRecord};
    use crate::protocol::{write_frame, DeltaFrame, Frame};

    const POLL: Duration = Duration::from_millis(500);
    loop {
        std::thread::sleep(POLL);
        for session in sessions.iter_mut() {
            let Ok(live) = session.volume.query_journal() else {
                write_frame(writer, &Frame::Resync)?;
                return Ok(());
            };
            if !cursor_is_usable(Some(session.journal), live) {
                write_frame(writer, &Frame::Resync)?;
                return Ok(());
            }
            let mut records = Vec::new();
            let Ok(read) =
                session
                    .volume
                    .read_usn(session.journal.next_usn, live.journal_id, |record| {
                        records.push(record);
                    })
            else {
                write_frame(writer, &Frame::Resync)?;
                return Ok(());
            };
            session.journal = JournalState {
                journal_id: live.journal_id,
                next_usn: read.next_usn,
            };
            for record in &records {
                let change = ChangeRecord {
                    file_id: record.file_id,
                    parent_id: record.parent_id,
                    name: record.name.clone(),
                    attributes: record.attributes,
                    reason: record.reason,
                };
                if let Some(delta) = delta_for(&mut session.dirs, &change) {
                    write_frame(
                        writer,
                        &Frame::Delta(DeltaFrame {
                            action: delta.action,
                            path: delta.path.to_string_lossy().into_owned(),
                        }),
                    )?;
                }
            }
        }
    }
}

/// Non-Windows builds have no named pipes; the helper is Windows-only.
#[cfg(not(target_os = "windows"))]
pub fn serve(_pipe_name: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the index helper requires Windows",
    ))
}

#[cfg(target_os = "windows")]
fn create_pipe(pipe_name: &str) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    // A LocalSystem service token's default DACL excludes the interactive user,
    // so resolve that user and grant it; standalone mode resolves `None` and
    // keeps the default DACL, which already covers the running user.
    let security = crate::security::PipeSecurity::for_active_session();
    let attributes = security
        .as_ref()
        .map_or(std::ptr::null(), |security| security.attributes());

    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            256 * 1024,
            256 * 1024,
            0,
            attributes,
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

/// Read one newline-terminated request line (control traffic is tiny).
#[cfg(target_os = "windows")]
fn read_request_line(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < MAX_REQUEST_BYTES {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return Ok(line);
        }
        line.push(byte[0]);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "request line too long",
    ))
}

/// The raw pipe handle, used both to read the request and to write the stream.
#[cfg(target_os = "windows")]
struct PipeStream {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl PipeStream {
    fn new(handle: windows_sys::Win32::Foundation::HANDLE) -> Self {
        Self { handle }
    }
}

#[cfg(target_os = "windows")]
impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use windows_sys::Win32::Storage::FileSystem::ReadFile;

        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                self.handle,
                buffer.as_mut_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }
}

#[cfg(target_os = "windows")]
impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        use windows_sys::Win32::Storage::FileSystem::WriteFile;

        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                self.handle,
                buffer.as_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    /// Writes on a byte-mode pipe are immediately visible to the reader, so
    /// there is nothing to flush (and `FlushFileBuffers` would block on a pipe).
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for PipeStream {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}
