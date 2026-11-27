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

/// Launch `path` through the Windows shell with the `runas` verb. Windows
/// shows its UAC prompt when the current process is not already elevated.
#[cfg(target_os = "windows")]
pub(crate) fn run_as_administrator(path: &std::path::Path) -> Result<()> {
    shell_execute(path, "runas")
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn run_as_administrator(path: &std::path::Path) -> Result<()> {
    let _ = path;
    anyhow::bail!("running as administrator is only supported on Windows")
}

/// Open `path` with its registered default program through the Windows shell.
#[cfg(target_os = "windows")]
pub(crate) fn open_file(path: &std::path::Path) -> Result<()> {
    shell_open(path)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_file(path: &std::path::Path) -> Result<()> {
    let _ = path;
    anyhow::bail!("opening files is not yet implemented on this platform")
}

/// Open Explorer with `path` selected.
#[cfg(target_os = "windows")]
pub(crate) fn reveal_in_explorer(path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn reveal_in_explorer(path: &std::path::Path) -> Result<()> {
    let _ = path;
    anyhow::bail!("revealing files is not yet implemented on this platform")
}

/// Open the Windows shell's Properties dialog for `path`.
#[cfg(target_os = "windows")]
pub(crate) fn show_properties(path: &std::path::Path) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{SHObjectProperties, SHOP_FILEPATH};

    let text = path.to_string_lossy();
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let shown = unsafe {
        SHObjectProperties(
            std::ptr::null_mut(),
            SHOP_FILEPATH as u32,
            wide.as_ptr(),
            std::ptr::null(),
        )
    };
    if shown == 0 {
        // Some shell namespace entries do not expose a file-properties page
        // through SHObjectProperties; retain the generic shell verb fallback.
        shell_execute(path, "properties")?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn show_properties(path: &std::path::Path) -> Result<()> {
    let _ = path;
    anyhow::bail!("showing file properties is not yet implemented on this platform")
}

/// Launch `path` through the Windows shell (`open` verb), which resolves
/// `.lnk` shortcuts, `shell:` namespace items and file associations the way
/// Explorer does. Returns an error when the shell reports a launch failure.
#[cfg(target_os = "windows")]
fn shell_open(path: &std::path::Path) -> Result<()> {
    shell_execute(path, "open")
}

/// Open `resource` with the exact installed application discovered by
/// Steward. Executables are started directly; shortcuts and shell application
/// aliases go through their registered shell handler with the resource as the
/// only parameter. A plugin never supplies a command line.
#[cfg(target_os = "windows")]
pub(crate) fn launch_with_resource(
    application: &std::path::Path,
    resource: &std::path::Path,
) -> Result<()> {
    use std::process::Command;

    let application_text = application.to_string_lossy();
    if application_text.to_ascii_lowercase().ends_with(".exe") {
        let mut command = Command::new(application);
        command.arg(resource);
        if let Some(directory) = application.parent() {
            command.current_dir(directory);
        }
        if command.spawn().is_ok() {
            return Ok(());
        }
    }
    shell_execute_with_parameters(application, "open", Some(resource))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn launch_with_resource(
    application: &std::path::Path,
    resource: &std::path::Path,
) -> Result<()> {
    let _ = (application, resource);
    anyhow::bail!("opening a path with an installed application is only supported on Windows")
}

#[cfg(target_os = "windows")]
fn shell_execute(path: &std::path::Path, verb: &str) -> Result<()> {
    shell_execute_with_parameters(path, verb, None)
}

#[cfg(target_os = "windows")]
fn shell_execute_with_parameters(
    path: &std::path::Path,
    verb: &str,
    parameter: Option<&std::path::Path>,
) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let text = path.to_string_lossy();
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = verb.encode_utf16().chain(std::iter::once(0)).collect();
    let parameters = parameter
        .map(|parameter| quote_windows_parameter(&parameter.to_string_lossy()))
        .transpose()?
        .map(|parameters| {
            parameters
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        });
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            wide.as_ptr(),
            parameters
                .as_ref()
                .map_or(std::ptr::null(), |parameters| parameters.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        anyhow::bail!("shell command failed for {}", text);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn quote_windows_parameter(parameter: &str) -> Result<String> {
    if parameter.contains('"') {
        anyhow::bail!("path contains a quote that cannot be passed safely");
    }
    if parameter.contains(' ') || parameter.contains('\t') {
        Ok(format!("\"{parameter}\""))
    } else {
        Ok(parameter.to_owned())
    }
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

/// Open a Windows Settings page through its registered `ms-settings:` URI.
/// Validating the URI scheme here prevents launcher actions from forwarding
/// arbitrary shell targets through this path.
#[cfg(target_os = "windows")]
pub(crate) fn open_settings(uri: &str) -> Result<()> {
    if !uri.starts_with("ms-settings:") {
        anyhow::bail!("invalid Windows Settings URI: {uri}");
    }
    shell_open(std::path::Path::new(uri))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_settings(uri: &str) -> Result<()> {
    let _ = uri;
    anyhow::bail!("opening Windows Settings is only supported on Windows")
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn settings_launch_is_rejected_off_windows() {
        assert!(super::open_settings("ms-settings:display").is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resource_parameters_are_quoted_without_shell_interpretation() {
        assert_eq!(
            super::quote_windows_parameter(r"D:\Projects\A & B\file.txt").unwrap(),
            r#""D:\Projects\A & B\file.txt""#
        );
        assert!(super::quote_windows_parameter("D:\\bad\"name.txt").is_err());
    }
}
