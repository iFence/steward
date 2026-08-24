//! Launch-at-logon registration (Windows per-user `Run` key).

/// Per-user autostart registry key (HKCU\...\Run). Values run at logon before
/// the shell starts; no admin rights are needed to write the current user's
/// key, and it travels with the user profile.
#[cfg(target_os = "windows")]
const AUTOSTART_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const AUTOSTART_VALUE_NAME: &str = "Steward";

/// Whether Steward is registered to launch at logon. Windows reads the
/// per-user `Run` key; other platforms are stubbed until M4.
#[cfg(target_os = "windows")]
pub(crate) fn autostart_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let path: Vec<u16> = AUTOSTART_REGISTRY_PATH
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name: Vec<u16> = AUTOSTART_VALUE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut data = [0u16; 1024];
    let mut size = (data.len() * 2) as u32;
    // ERROR_SUCCESS (0) means the value exists; a non-empty string also
    // guards against a value that is present but blank.
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
    result == 0 && size >= 2 && data[0] != 0
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn autostart_enabled() -> bool {
    false
}

/// Register (`enabled`) or unregister Steward at logon and return the state
/// that actually took effect, so the tray check mark always mirrors reality.
#[cfg(target_os = "windows")]
pub(crate) fn set_autostart(enabled: bool) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    let path: Vec<u16> = AUTOSTART_REGISTRY_PATH
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name: Vec<u16> = AUTOSTART_VALUE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let result = if enabled {
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("autostart: cannot resolve the current executable path");
            return autostart_enabled();
        };
        let value: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr()) }
    };
    if result != 0 {
        eprintln!("autostart: registry update failed with error 0x{result:x}");
    }
    autostart_enabled()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_autostart(_enabled: bool) -> bool {
    false
}
