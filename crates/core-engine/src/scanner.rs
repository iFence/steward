//! Application discovery.
//!
//! M1 targets Windows: scan the per-user and all-users Start Menu for `.lnk`
//! shortcuts and resolve their targets via the ShellLink COM interface.
//! Other platforms are stubbed until later milestones.

use std::path::PathBuf;

use crate::AppEntry;

/// Platform abstraction for discovering installed applications.
pub trait AppScanner {
    fn scan(&self) -> Vec<AppEntry>;
}

/// Returns the active scanner for the current platform. On Windows this scans
/// the Start Menu; on other platforms it returns an empty, no-op scanner.
pub fn platform_scanner() -> Box<dyn AppScanner> {
    #[cfg(target_os = "windows")]
    {
        Box::new(imp::WinAppsScanner::new())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Box::new(imp::NoopScanner)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;
    use windows::core::Interface;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        STGM_READ,
    };
    use windows::Win32::UI::Shell::IShellLinkW;

    /// CLSID_ShellLink — `{00021401-0000-0000-C000-000000000046}`. The
    /// `windows` crate no longer ships this constant, so it is declared here.
    const CLSID_SHELL_LINK: windows::core::GUID =
        windows::core::GUID::from_u128(0x0002140100000000C000000000000046);

    /// Windows Start Menu scanner.
    pub struct WinAppsScanner;

    impl WinAppsScanner {
        pub fn new() -> Self {
            Self
        }

        fn roots() -> Vec<PathBuf> {
            let mut roots = Vec::new();
            if let Some(p) = std::env::var_os("APPDATA") {
                roots.push(
                    PathBuf::from(p)
                        .join("Microsoft")
                        .join("Windows")
                        .join("Start Menu")
                        .join("Programs"),
                );
            }
            if let Some(p) = std::env::var_os("PROGRAMDATA") {
                roots.push(
                    PathBuf::from(p)
                        .join("Microsoft")
                        .join("Windows")
                        .join("Start Menu")
                        .join("Programs"),
                );
            }
            roots
        }
    }

    fn collect_lnks(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_lnks(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            {
                out.push(path);
            }
        }
    }

    /// Resolve a `.lnk` file to its target path via the ShellLink COM object.
    fn resolve_lnk(lnk: &Path) -> Option<String> {
        // SAFETY: `shell_link` owns a COM reference until it is dropped; the
        // value returned by `CoCreateInstance` is released automatically.
        unsafe {
            let shell_link: IShellLinkW =
                CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistFile = shell_link.cast().ok()?;

            let wide = widestring::U16CString::from_os_str(lnk.as_os_str()).ok()?;
            persist.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;

            // GetPath fills the wide buffer; pre-zero it so we can locate the
            // first null terminator (the wrapper does not report the length).
            // Flags 0 (SLGP_UNCPRIORITY) expand environment variables such as
            // %windir% in shortcut targets; SLGP_RAWPATH would keep them
            // unexpanded and break both launching and icon extraction.
            let mut buf = [0u16; 1024];
            shell_link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    impl AppScanner for WinAppsScanner {
        fn scan(&self) -> Vec<AppEntry> {
            // SAFETY: initialise COM once for this thread.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }

            let mut lnks = Vec::new();
            for root in Self::roots() {
                collect_lnks(&root, &mut lnks);
            }

            let mut seen: HashSet<String> = HashSet::new();
            let mut apps = Vec::new();
            for lnk in lnks {
                let Some(target) = resolve_lnk(&lnk) else {
                    continue;
                };
                // Keep only direct executable targets.
                let is_exe = Path::new(&target)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
                if !is_exe || !seen.insert(target.clone()) {
                    continue;
                }
                let name = lnk
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| target.clone());
                apps.push(AppEntry {
                    name,
                    path: PathBuf::from(target),
                });
            }
            apps
        }
    }

    impl Default for WinAppsScanner {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::*;

    /// No-op scanner for platforms not yet implemented.
    pub struct NoopScanner;

    impl AppScanner for NoopScanner {
        fn scan(&self) -> Vec<AppEntry> {
            Vec::new()
        }
    }
}
