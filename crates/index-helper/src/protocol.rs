//! Binary framing for the privileged index helper.
//!
//! Control is a single JSON request line; the index itself is streamed as
//! length-prefixed binary frames. JSON per record would cost hundreds of bytes
//! and a parse per entry, which is the wrong shape for a multi-million-entry
//! volume, so the bulk path is a fixed little-endian layout:
//!
//! ```text
//! frame := u32 payload_len | payload
//! payload := kind:u8 | kind-specific fields
//! ```

use std::io::{self, Read, Write};

/// Version of both the request line and the frame layout.
///
/// Version 2 adds the delta frames; the version-1 frames are unchanged, so a
/// client can still read a snapshot from an older helper and vice versa.
pub const PROTOCOL_VERSION: u32 = 2;
/// Default pipe the helper listens on.
pub const PIPE_NAME: &str = r"\\.\pipe\steward-index-v1";
/// Names are capped the same way the in-memory index caps them.
pub const MAX_NAME_BYTES: usize = 4096;
/// Upper bound for one frame, so a corrupt length cannot allocate wildly.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

const KIND_VOLUME: u8 = 1;
const KIND_RECORD: u8 = 2;
const KIND_END: u8 = 3;
const KIND_DELTA: u8 = 4;
const KIND_RESYNC: u8 = 5;

const FLAG_DIR: u8 = 0b0000_0001;
const FLAG_REPARSE: u8 = 0b0000_0010;
const FLAG_SIZE_VALID: u8 = 0b0000_0100;
/// The record is a volume root (or configured folder root); the client links it
/// as a root rather than as a child of another record.
pub const FLAG_ROOT: u8 = 0b1000_0000;

/// Which enumerator produced a volume's records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Direct `$MFT` parsing, the elevated fast path.
    Mft,
    /// Recursive directory enumeration.
    Walk,
}

impl Backend {
    fn as_u8(self) -> u8 {
        match self {
            Backend::Mft => 0,
            Backend::Walk => 1,
        }
    }

    fn from_u8(value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Backend::Mft),
            1 => Ok(Backend::Walk),
            other => Err(ProtocolError::BadBackend(other)),
        }
    }
}

/// Header for one volume, sent before that volume's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeFrame {
    /// Upper-case drive letter, or `0` for a configured folder root.
    pub drive: u8,
    pub backend: Backend,
    pub root_name: String,
}

/// One index record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFrame {
    pub id: u64,
    pub parent_id: u64,
    pub size: Option<u64>,
    pub mtime: Option<u64>,
    pub attributes: u32,
    pub name: String,
    pub is_dir: bool,
    pub is_reparse: bool,
    /// The root of the volume / configured root, linked with `add_root`.
    pub is_root: bool,
}

/// Terminates a stream and reports what it contained.
///
/// In snapshot-only mode (a walk root, or a volume without a usable journal)
/// the connection closes after this. When a volume can be maintained from its
/// USN journal, [`Frame::Delta`] frames follow and the connection stays open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EndFrame {
    pub records: u64,
    pub directories: u64,
}

/// One live filesystem change, resolved to an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaFrame {
    pub action: crate::delta::DeltaAction,
    pub path: String,
}

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Volume(VolumeFrame),
    Record(RecordFrame),
    End(EndFrame),
    Delta(DeltaFrame),
    /// The journal is no longer usable; the client must rebuild (reconnect).
    Resync,
}

/// Why a frame could not be decoded.
#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    Truncated,
    BadKind(u8),
    BadBackend(u8),
    BadAction(u8),
    FrameTooLarge(usize),
    InvalidUtf8,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "i/o error: {error}"),
            Self::Truncated => f.write_str("frame is truncated"),
            Self::BadKind(kind) => write!(f, "unknown frame kind {kind}"),
            Self::BadBackend(value) => write!(f, "unknown backend {value}"),
            Self::BadAction(value) => write!(f, "unknown delta action {value}"),
            Self::FrameTooLarge(len) => write!(f, "frame length {len} is out of range"),
            Self::InvalidUtf8 => f.write_str("frame carries invalid UTF-8"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Encode a frame's payload (without the length prefix).
fn encode_payload(frame: &Frame) -> Vec<u8> {
    let mut encoder = Encoder::default();
    match frame {
        Frame::Volume(volume) => {
            encoder.u8(KIND_VOLUME);
            encoder.u8(volume.drive);
            encoder.u8(volume.backend.as_u8());
            encoder.text(&volume.root_name);
        }
        Frame::Record(record) => {
            encoder.u8(KIND_RECORD);
            let mut flags = 0u8;
            if record.is_dir {
                flags |= FLAG_DIR;
            }
            if record.is_reparse {
                flags |= FLAG_REPARSE;
            }
            if record.size.is_some() {
                flags |= FLAG_SIZE_VALID;
            }
            if record.is_root {
                flags |= FLAG_ROOT;
            }
            encoder.u8(flags);
            encoder.u64(record.id);
            encoder.u64(record.parent_id);
            encoder.u64(record.size.unwrap_or(0));
            encoder.u64(record.mtime.unwrap_or(0));
            encoder.u32(record.attributes);
            encoder.text(&record.name);
        }
        Frame::End(end) => {
            encoder.u8(KIND_END);
            encoder.u64(end.records);
            encoder.u64(end.directories);
        }
        Frame::Delta(delta) => {
            encoder.u8(KIND_DELTA);
            encoder.u8(action_to_u8(delta.action));
            // A full path exceeds the 4096-byte name cap a record name gets, so
            // it uses the plain u16 length encoding.
            encoder.long_text(&delta.path);
        }
        Frame::Resync => {
            encoder.u8(KIND_RESYNC);
        }
    }
    encoder.bytes
}

/// Decode a frame payload.
fn decode_payload(payload: &[u8]) -> Result<Frame, ProtocolError> {
    let mut decoder = Decoder::new(payload);
    let kind = decoder.u8()?;
    match kind {
        KIND_VOLUME => {
            let drive = decoder.u8()?;
            let backend = Backend::from_u8(decoder.u8()?)?;
            let root_name = decoder.text()?;
            Ok(Frame::Volume(VolumeFrame {
                drive,
                backend,
                root_name,
            }))
        }
        KIND_RECORD => {
            let flags = decoder.u8()?;
            let id = decoder.u64()?;
            let parent_id = decoder.u64()?;
            let size = decoder.u64()?;
            let mtime = decoder.u64()?;
            let attributes = decoder.u32()?;
            let name = decoder.text()?;
            Ok(Frame::Record(RecordFrame {
                id,
                parent_id,
                size: (flags & FLAG_SIZE_VALID != 0).then_some(size),
                mtime: (mtime != 0).then_some(mtime),
                attributes,
                name,
                is_dir: flags & FLAG_DIR != 0,
                is_reparse: flags & FLAG_REPARSE != 0,
                is_root: flags & FLAG_ROOT != 0,
            }))
        }
        KIND_END => Ok(Frame::End(EndFrame {
            records: decoder.u64()?,
            directories: decoder.u64()?,
        })),
        KIND_DELTA => Ok(Frame::Delta(DeltaFrame {
            action: action_from_u8(decoder.u8()?)?,
            path: decoder.text()?,
        })),
        KIND_RESYNC => Ok(Frame::Resync),
        other => Err(ProtocolError::BadKind(other)),
    }
}

fn action_to_u8(action: crate::delta::DeltaAction) -> u8 {
    match action {
        crate::delta::DeltaAction::Created => 0,
        crate::delta::DeltaAction::Removed => 1,
        crate::delta::DeltaAction::Modified => 2,
        crate::delta::DeltaAction::RenamedOld => 3,
        crate::delta::DeltaAction::RenamedNew => 4,
    }
}

fn action_from_u8(value: u8) -> Result<crate::delta::DeltaAction, ProtocolError> {
    match value {
        0 => Ok(crate::delta::DeltaAction::Created),
        1 => Ok(crate::delta::DeltaAction::Removed),
        2 => Ok(crate::delta::DeltaAction::Modified),
        3 => Ok(crate::delta::DeltaAction::RenamedOld),
        4 => Ok(crate::delta::DeltaAction::RenamedNew),
        other => Err(ProtocolError::BadAction(other)),
    }
}

/// Write one length-prefixed frame.
pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let payload = encode_payload(frame);
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(())
}

/// Read one length-prefixed frame. `Ok(None)` means the peer closed cleanly.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, ProtocolError> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(ProtocolError::Io(error)),
    }
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    let mut payload = vec![0u8; len];
    if let Err(error) = reader.read_exact(&mut payload) {
        return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
            ProtocolError::Truncated
        } else {
            ProtocolError::Io(error)
        });
    }
    decode_payload(&payload).map(Some)
}

#[derive(Default)]
struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// A `u16` length followed by the UTF-8 bytes.
    fn text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let len = bytes.len().min(MAX_NAME_BYTES) as u16;
        self.bytes.extend_from_slice(&len.to_le_bytes());
        self.bytes.extend_from_slice(&bytes[..len as usize]);
    }

    /// A `u16` length followed by the UTF-8 bytes, without the name cap. Used
    /// for full paths, which are longer than a single record name.
    fn long_text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut len = bytes.len().min(u16::MAX as usize);
        while len > 0 && !text.is_char_boundary(len) {
            len -= 1;
        }
        self.bytes.extend_from_slice(&(len as u16).to_le_bytes());
        self.bytes.extend_from_slice(&bytes[..len]);
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.at.checked_add(len).ok_or(ProtocolError::Truncated)?;
        if end > self.bytes.len() {
            return Err(ProtocolError::Truncated);
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    fn text(&mut self) -> Result<String, ProtocolError> {
        let len = u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")) as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame: Frame) {
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        let decoded = read_frame(&mut wire.as_slice()).unwrap().unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn records_round_trip() {
        round_trip(Frame::Record(RecordFrame {
            id: 42,
            parent_id: 7,
            size: Some(1024),
            mtime: Some(99),
            attributes: 0x20,
            name: "报告.txt".into(),
            is_dir: false,
            is_reparse: false,
            is_root: false,
        }));
    }

    #[test]
    fn roots_and_directories_round_trip() {
        round_trip(Frame::Record(RecordFrame {
            id: 5,
            parent_id: 0,
            size: None,
            mtime: None,
            attributes: 0x10,
            name: "C:\\".into(),
            is_dir: true,
            is_reparse: false,
            is_root: true,
        }));
    }

    #[test]
    fn volume_and_end_round_trip() {
        round_trip(Frame::Volume(VolumeFrame {
            drive: b'C',
            backend: Backend::Mft,
            root_name: "C:\\".into(),
        }));
        round_trip(Frame::End(EndFrame {
            records: 12,
            directories: 3,
        }));
    }

    #[test]
    fn delta_and_resync_round_trip() {
        round_trip(Frame::Delta(DeltaFrame {
            action: crate::delta::DeltaAction::RenamedNew,
            path: "C:\\Users\\报告.txt".into(),
        }));
        round_trip(Frame::Delta(DeltaFrame {
            action: crate::delta::DeltaAction::Removed,
            path: "D:\\a\\b".into(),
        }));
        round_trip(Frame::Resync);
    }

    #[test]
    fn a_path_longer_than_a_record_name_round_trips() {
        // Record names cap at 4096 bytes, but a full path can be longer; the
        // delta encoding must not truncate it at the name cap.
        let path = format!("C:\\{}", "x".repeat(5000));
        round_trip(Frame::Delta(DeltaFrame {
            action: crate::delta::DeltaAction::Created,
            path,
        }));
    }

    #[test]
    fn clean_eof_is_none() {
        let mut empty: &[u8] = &[];
        assert!(read_frame(&mut empty).unwrap().is_none());
    }

    #[test]
    fn oversized_and_truncated_frames_are_rejected() {
        let mut oversized = ((MAX_FRAME_BYTES as u32) + 1).to_le_bytes().to_vec();
        oversized.push(0);
        assert!(matches!(
            read_frame(&mut oversized.as_slice()),
            Err(ProtocolError::FrameTooLarge(_))
        ));

        let truncated = [8u8, 0, 0, 0, KIND_RECORD, 0];
        assert!(matches!(
            read_frame(&mut truncated.as_slice()),
            Err(ProtocolError::Truncated)
        ));
    }
}
