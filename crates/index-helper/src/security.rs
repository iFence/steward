//! Pipe security for service mode.
//!
//! A LocalSystem service creates its named pipe with the *service token's*
//! default DACL, which grants SYSTEM and Administrators but not the interactive
//! user, so the app could not connect. This module resolves the active console
//! session's user SID and builds an SDDL descriptor that grants SYSTEM,
//! Administrators and that user.
//!
//! Resolution needs `SE_TCB_NAME` (`WTSQueryUserToken`), which only the service
//! token has. When it fails (a standalone non-elevated helper, or a session
//! switch in flight) the caller keeps the default DACL, which is exactly right
//! for a process running as that user; the app then falls back to its own walk
//! if it cannot connect.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_USER,
};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};

/// A security descriptor plus the `SECURITY_ATTRIBUTES` that reference it.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    /// Build a descriptor for the active console session's user.
    ///
    /// `None` means "keep the default DACL": the caller is running as a user
    /// (standalone mode) or the session could not be resolved.
    pub fn for_active_session() -> Option<Self> {
        // SAFETY: every call below is a documented Win32 API used with
        // correctly sized buffers; every allocation is freed with the matching
        // `LocalFree`/`CloseHandle`, and the descriptor is copied into the pipe
        // by `CreateNamedPipeW` before this value is dropped.
        unsafe {
            let session = WTSGetActiveConsoleSessionId();
            if session == u32::MAX {
                return None;
            }
            let mut token: HANDLE = ptr::null_mut();
            if WTSQueryUserToken(session, &mut token) == 0 {
                return None;
            }
            let security = build(token);
            CloseHandle(token);
            security
        }
    }

    /// The `lpSecurityAttributes` argument for `CreateNamedPipeW`.
    pub fn attributes(&self) -> *const SECURITY_ATTRIBUTES {
        &self.attributes
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: `descriptor` came from
        // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which
        // allocates it with `LocalAlloc`.
        unsafe {
            let _ = LocalFree(self.descriptor.cast());
        }
    }
}

/// Read the token's user SID and turn it into an SDDL descriptor.
///
/// # Safety
/// `token` must be a valid token handle with `TOKEN_QUERY`.
unsafe fn build(token: HANDLE) -> Option<PipeSecurity> {
    let mut size = 0u32;
    // The first call sizes the buffer and is expected to fail.
    GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
    if size == 0 {
        return None;
    }
    // `Vec<usize>` keeps the buffer pointer-aligned, which `TOKEN_USER` needs;
    // a `Vec<u8>` would only be byte-aligned.
    let words = (size as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer: Vec<usize> = vec![0; words];
    if GetTokenInformation(
        token,
        TokenUser,
        buffer.as_mut_ptr().cast(),
        size,
        &mut size,
    ) == 0
    {
        return None;
    }
    let user = &*(buffer.as_ptr() as *const TOKEN_USER);

    let mut sid_string: *mut u16 = ptr::null_mut();
    if ConvertSidToStringSidW(user.User.Sid, &mut sid_string) == 0 {
        return None;
    }
    let sid_text = wide_to_string(sid_string);
    let _ = LocalFree(sid_string.cast());
    let sid_text = sid_text?;

    descriptor_from_sddl(&sddl_for_user(&sid_text))
}

/// The DACL a service-mode pipe gets: SYSTEM, Administrators and the console
/// user, nothing else.
///
/// The user is in the same trust domain as the index (it is their own machine's
/// metadata), so full control is fine and avoids reading back the exact access
/// mask the app needs.
fn sddl_for_user(sid: &str) -> String {
    format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})")
}

/// Convert an SDDL string into a `SECURITY_ATTRIBUTES` plus its descriptor.
///
/// # Safety
/// Calls Win32 allocation APIs; the descriptor is owned by the returned value.
unsafe fn descriptor_from_sddl(sddl: &str) -> Option<PipeSecurity> {
    let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    if ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl_wide.as_ptr(),
        SDDL_REVISION_1,
        &mut descriptor,
        ptr::null_mut(),
    ) == 0
    {
        return None;
    }
    Some(PipeSecurity {
        descriptor,
        attributes: SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        },
    })
}

/// # Safety
/// `pointer` must be a NUL-terminated UTF-16 string.
unsafe fn wide_to_string(pointer: *const u16) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *pointer.add(len) != 0 {
        len += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(pointer, len)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_dacl_names_system_admins_and_the_user() {
        let sddl = sddl_for_user("S-1-5-21-1-2-3-1001");
        assert!(sddl.contains(";;;SY)"));
        assert!(sddl.contains(";;;BA)"));
        assert!(sddl.ends_with(";;;S-1-5-21-1-2-3-1001)"));
    }

    /// Exercises the real SDDL conversion (`advapi32`) without needing a service
    /// token: a fabricated but well-formed SID is enough.
    #[test]
    fn a_user_sddl_converts_to_a_descriptor() {
        let security = unsafe { descriptor_from_sddl(&sddl_for_user("S-1-5-21-1-2-3-1001")) }
            .expect("a well-formed SDDL must convert");
        let attributes = security.attributes();
        assert!(!attributes.is_null());
        assert_eq!(
            unsafe { (*attributes).nLength },
            std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32
        );
        assert!(!unsafe { (*attributes).lpSecurityDescriptor }.is_null());
    }
}
