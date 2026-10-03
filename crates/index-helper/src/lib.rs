//! Optional privileged index helper.
//!
//! Steward's UI process runs with the user's token, so it cannot open `\\.\X:`
//! to read the `$MFT`. This helper can (when launched elevated or as a service),
//! enumerates a volume at MFT speed and streams the records to the app over a
//! named pipe. Without the helper the app falls back to its own directory walk,
//! so the helper is an accelerator, never a dependency.

pub mod delta;
pub mod protocol;
pub mod source;

#[cfg(target_os = "windows")]
pub mod client;
#[cfg(target_os = "windows")]
pub mod security;
#[cfg(target_os = "windows")]
pub mod server;
#[cfg(target_os = "windows")]
pub mod service;

pub use delta::{Delta, DeltaAction, DirIndex};
pub use protocol::{Backend, Frame, PIPE_NAME, PROTOCOL_VERSION};
pub use source::{StreamRequest, Summary, DEFAULT_EXCLUDED_DIRS};
