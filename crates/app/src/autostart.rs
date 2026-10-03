//! Launch-at-logon registration (Windows per-user `Run` key).

#[cfg(any(target_os = "windows", test))]
use std::path::{Path, PathBuf};

/// Per-user autostart registry key (HKCU\...\Run). Values run at logon before
/// the shell starts; no admin rights are needed to write the current user's
/// key, and it travels with the user profile.
#[cfg(target_os = "windows")]
const AUTOSTART_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const AUTOSTART_VALUE_NAME: &str = "Steward";

/// `ERROR_FILE_NOT_FOUND`, returned by `RegDeleteKeyValueW` when the value is
/// already gone. Deleting an absent autostart entry is a success.
#[cfg(target_os = "windows")]
const ERROR_FILE_NOT_FOUND: u32 = 2;

/// Quote a path for the `Run` value. Windows parses an unquoted path with
/// spaces by trying shorter prefixes first, so the executable path is always
/// stored quoted (`"C:\Program Files\Steward\steward-app.exe"`).
#[cfg(any(target_os = "windows", test))]
fn quote_autostart_value(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// Parse a `Run` value into the executable path it points at, stripping the
/// surrounding quotes the value is normally stored with.
#[cfg(any(target_os = "windows", test))]
fn parse_autostart_value(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed);
    PathBuf::from(unquoted)
}

/// Whether Steward is registered to launch at logon. The switch mirrors the
/// registry: it is on whenever a non-empty value exists, even if that value
/// still points at an older build (the installer and startup repair keep it
/// current). Other platforms are stubbed until M4.
#[cfg(target_os = "windows")]
pub(crate) fn autostart_enabled() -> bool {
    read_autostart_value().is_some()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn autostart_enabled() -> bool {
    false
}

/// Register (`enabled`) or unregister Steward at logon and return the state
/// that actually took effect, so the settings switch always mirrors reality.
#[cfg(target_os = "windows")]
pub(crate) fn set_autostart(enabled: bool) -> bool {
    if enabled {
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("autostart: cannot resolve the current executable path");
            return autostart_enabled();
        };
        let result = write_autostart_value(&quote_autostart_value(&exe));
        if result != 0 {
            eprintln!("autostart: registry update failed with error 0x{result:x}");
        }
    } else {
        let result = delete_autostart_value();
        if result != 0 && result != ERROR_FILE_NOT_FOUND {
            eprintln!("autostart: registry delete failed with error 0x{result:x}");
        }
    }
    autostart_enabled()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_autostart(_enabled: bool) -> bool {
    false
}

/// Point an existing autostart entry at the current executable. The installer
/// runs this after `InstallFiles`, so a freshly installed build owns the logon
/// entry even when the old value pointed at a different (dev) build. Does
/// nothing when autostart is off, preserving an explicit opt-out.
#[cfg(target_os = "windows")]
pub(crate) fn sync_autostart_path() -> bool {
    let Some(raw) = read_autostart_value() else {
        return false;
    };
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("autostart: cannot resolve the current executable path");
        return false;
    };
    let quoted = quote_autostart_value(&exe);
    if raw.trim() == quoted {
        return false;
    }
    let result = write_autostart_value(&quoted);
    if result != 0 {
        eprintln!("autostart: registry update failed with error 0x{result:x}");
        return false;
    }
    true
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn sync_autostart_path() -> bool {
    false
}

/// Repair a dangling entry left behind by a moved or deleted build: when the
/// recorded executable no longer exists, point the entry at the running
/// executable. A still-existing other build (e.g. a local `target\debug`
/// copy) is left untouched, so a dev run cannot steal the installed build's
/// autostart entry.
#[cfg(target_os = "windows")]
pub(crate) fn repair_autostart_path() -> bool {
    let Some(raw) = read_autostart_value() else {
        return false;
    };
    if parse_autostart_value(&raw).is_file() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let result = write_autostart_value(&quote_autostart_value(&exe));
    if result != 0 {
        eprintln!("autostart: registry repair failed with error 0x{result:x}");
        return false;
    }
    true
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn repair_autostart_path() -> bool {
    false
}

/// Remove the per-user autostart entry. The uninstaller runs this before
/// `RemoveFiles`; succeed when the value was already absent.
#[cfg(target_os = "windows")]
pub(crate) fn unregister_autostart() -> bool {
    let result = delete_autostart_value();
    if result != 0 && result != ERROR_FILE_NOT_FOUND {
        eprintln!("autostart: registry delete failed with error 0x{result:x}");
        return false;
    }
    !autostart_enabled()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn unregister_autostart() -> bool {
    true
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read the raw `Run` value, or `None` when it is absent or blank.
#[cfg(target_os = "windows")]
fn read_autostart_value() -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let path = wide(AUTOSTART_REGISTRY_PATH);
    let name = wide(AUTOSTART_VALUE_NAME);

    // First call with a null buffer reports the required size (bytes,
    // including the terminating NUL) without copying anything.
    let mut size = 0u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if result != 0 || size < 2 {
        return None;
    }

    let mut data = vec![0u16; size.div_ceil(2) as usize];
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return None;
    }

    let mut len = size as usize / 2;
    while len > 0 && data[len - 1] == 0 {
        len -= 1;
    }
    let value = String::from_utf16_lossy(&data[..len]);
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Write the `Run` value and return the Win32 result code (0 is success).
#[cfg(target_os = "windows")]
fn write_autostart_value(value: &str) -> u32 {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let path = wide(AUTOSTART_REGISTRY_PATH);
    let name = wide(AUTOSTART_VALUE_NAME);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    }
}

/// Delete the `Run` value and return the Win32 result code (0 is success).
#[cfg(target_os = "windows")]
fn delete_autostart_value() -> u32 {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};

    let path = wide(AUTOSTART_REGISTRY_PATH);
    let name = wide(AUTOSTART_VALUE_NAME);
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_unquotes_the_executable_path() {
        let path = Path::new(r"C:\Program Files\Steward\steward-app.exe");
        let quoted = quote_autostart_value(path);
        assert_eq!(quoted, r#""C:\Program Files\Steward\steward-app.exe""#);
        assert_eq!(parse_autostart_value(&quoted), PathBuf::from(path));
    }

    #[test]
    fn parses_unquoted_and_padded_values() {
        assert_eq!(
            parse_autostart_value("  D:\\tools\\steward-app.exe  "),
            PathBuf::from(r"D:\tools\steward-app.exe")
        );
    }

    #[test]
    fn keeps_paths_with_spaces_intact() {
        assert_eq!(
            parse_autostart_value(r#""C:\Users\a b\Steward\steward-app.exe""#),
            PathBuf::from(r"C:\Users\a b\Steward\steward-app.exe")
        );
    }
}
