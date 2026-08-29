//! Spawning external applications.

use anyhow::Result;

/// Spawn the application at `path`, detaching from the launcher process so
/// both keep running independently.
pub(crate) fn launch(path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let text = path.to_string_lossy();
        // `.lnk` shortcuts (shell namespace items such as Control Panel) and
        // `shell:` UWP aliases can't be spawned via CreateProcess; let the
        // shell resolve them.
        if text.starts_with("shell:") || text.to_ascii_lowercase().ends_with(".lnk") {
            return shell_open(path);
        }
        // Start the child in its own directory, matching what Explorer does:
        // many Windows apps (self-contained .NET, Electron, ...) resolve
        // configuration and adjacent DLLs relative to the working directory
        // and silently exit when launched from an unrelated cwd — a common
        // regression after an app updates its packaging.
        let mut command = Command::new(path);
        if let Some(dir) = path.parent() {
            command.current_dir(dir);
        }
        match command.spawn() {
            // Keep the child detached by not holding a handle to it.
            Ok(_child) => Ok(()),
            // CreateProcess rejects some targets that the shell resolves fine
            // (file associations, argument-carrying shortcuts, elevation
            // prompts); fall back to the shell before giving up.
            Err(_) => shell_open(path),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows launching is stubbed for M1.
        let _ = path;
        anyhow::bail!("launching is not yet implemented on this platform");
    }
}

/// Launch `path` through the Windows shell (`open` verb), which resolves
/// `.lnk` shortcuts, `shell:` namespace items and file associations the way
/// Explorer does. Returns an error when the shell reports a launch failure.
#[cfg(target_os = "windows")]
fn shell_open(path: &std::path::Path) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let text = path.to_string_lossy();
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        anyhow::bail!("shell launch failed for {}", text);
    }
    Ok(())
}

/// Open `url` in the user's default browser. The `open` verb routes `http` /
/// `https` URLs to the registered default handler, so the launcher never has
/// to locate a browser.
#[cfg(target_os = "windows")]
pub(crate) fn open_url(url: &str) -> Result<()> {
    // A scheme-less address (e.g. `172.20.2.14:1230`) would be read by the
    // Windows shell as a file path, not a URL, so it would never reach the
    // browser. Supply the `http://` scheme — the same normalization a browser
    // address bar applies to a bare `host:port` — so it routes correctly.
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    };
    shell_open(std::path::Path::new(&url))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_url(url: &str) -> Result<()> {
    let _ = url;
    anyhow::bail!("opening URLs is not yet implemented on this platform")
}

/// Open `path` (a file, folder, or `shell:`/`.lnk` alias) with the OS default
/// handler — the `open` verb, the same way Explorer resolves the target. Used
/// by plugins granted the `open.path` permission.
#[cfg(target_os = "windows")]
pub(crate) fn open_path(path: &str) -> Result<()> {
    shell_open(std::path::Path::new(path))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_path(path: &str) -> Result<()> {
    let _ = path;
    anyhow::bail!("opening paths is not yet implemented on this platform")
}
