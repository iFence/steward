//! Windows Explorer discovery and address-bar-only dialog navigation.
//!
//! Never write the filename control, click IDOK, or use the clipboard.

use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{bail, ensure, Result};
use windows::{
    core::Interface,
    Win32::{
        System::{
            Com::{CoCreateInstance, CoTaskMemFree, IServiceProvider, CLSCTX_ALL},
            Variant::VARIANT,
        },
        UI::Shell::{
            IFolderView, IPersistFolder2, IShellBrowser, IShellWindows, IWebBrowserApp,
            SHGetNameFromIDList, SID_STopLevelBrowser, ShellWindows, SIGDN_FILESYSPATH,
        },
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, IsWindowEnabled, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD,
            KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_MENU, VK_RETURN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            EnumChildWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
            GetParent, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
            SendMessageTimeoutW, SetForegroundWindow, GA_ROOT, GUITHREADINFO, SMTO_ABORTIFHUNG,
            WM_KEYDOWN, WM_KEYUP, WM_SETTEXT,
        },
    },
};

fn class_name(hwnd: HWND) -> String {
    let mut text = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, text.as_mut_ptr(), text.len() as i32) };
    String::from_utf16_lossy(&text[..len.max(0) as usize])
}

fn children(hwnd: HWND) -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> i32 {
        // The Vec lives for the synchronous EnumChildWindows call.
        unsafe { &mut *(data as *mut Vec<HWND>) }.push(hwnd);
        1
    }
    let mut result = Vec::new();
    unsafe { EnumChildWindows(hwnd, Some(collect), &mut result as *mut _ as LPARAM) };
    result
}

fn has_ancestor_class(mut hwnd: HWND, root: HWND, class: &str) -> bool {
    while !hwnd.is_null() && hwnd != root {
        hwnd = unsafe { GetParent(hwnd) };
        if hwnd != root && class_name(hwnd) == class {
            return true;
        }
    }
    false
}

/// An identity captured BEFORE Steward takes focus. Handles never cross into
/// COM; the integers can be sent to the navigation worker safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DialogTarget {
    hwnd: usize,
    process: u32,
    thread: u32,
    address: usize,
}

impl DialogTarget {
    pub(crate) fn foreground() -> Option<Self> {
        Self::capture(unsafe { GetForegroundWindow() })
    }

    fn capture(hwnd: HWND) -> Option<Self> {
        if hwnd.is_null()
            || class_name(hwnd) != "#32770"
            || unsafe { IsWindowVisible(hwnd) == 0 || IsWindowEnabled(hwnd) == 0 }
        {
            return None;
        }
        let descendants = children(hwnd);
        // #32770 alone also matches message boxes. Require both the Shell
        // folder view and its address ComboBoxEx32; fail closed for custom UI.
        if !descendants
            .iter()
            .any(|&h| class_name(h) == "SHELLDLL_DefView")
        {
            return None;
        }
        let address = descendants.into_iter().find(|&h| {
            class_name(h) == "ComboBoxEx32" && has_ancestor_class(h, hwnd, "Address Band Root")
        })?;
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
        if thread == 0 || process == 0 {
            return None;
        }
        Some(Self {
            hwnd: hwnd as usize,
            process,
            thread,
            address: address as usize,
        })
    }

    pub(crate) fn valid(self) -> bool {
        (unsafe { IsWindow(self.hwnd as HWND) != 0 })
            && Self::capture(self.hwnd as HWND) == Some(self)
    }

    pub(crate) fn is_foreground(self) -> bool {
        self.valid() && unsafe { GetForegroundWindow() as usize == self.hwnd }
    }

    /// Identity-only target for tests. No window stands behind it, so every
    /// live check on it reports a dead dialog.
    #[cfg(test)]
    pub(crate) fn for_test(hwnd: usize) -> Self {
        Self {
            hwnd,
            process: 1,
            thread: 1,
            address: 1,
        }
    }

    /// Screen rect of the dialog in physical pixels, for pinning the launcher
    /// bar flush under its bottom edge. `None` once the dialog is gone or has
    /// no usable frame (e.g. minimized).
    pub(crate) fn rect(self) -> Option<crate::platform::Rect> {
        if unsafe { IsWindow(self.hwnd as HWND) } == 0 {
            return None;
        }
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(self.hwnd as HWND, &mut rect) } == 0 {
            return None;
        }
        (rect.right > rect.left && rect.bottom > rect.top).then_some(crate::platform::Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    /// Width of the dialog in logical (GPUI) pixels, measured at the dialog's
    /// own DPI: the launcher matches it so the bar lines up with the dialog's
    /// edges on any display scale.
    pub(crate) fn logical_width(self) -> Option<f32> {
        let rect = self.rect()?;
        let dpi = unsafe { GetDpiForWindow(self.hwnd as HWND) }.max(96);
        Some((rect.right - rect.left) as f32 * 96.0 / dpi as f32)
    }

    pub(crate) fn restore_focus(self) -> bool {
        if !self.valid() {
            return false;
        }
        unsafe { SetForegroundWindow(self.hwnd as HWND) };
        self.is_foreground()
    }

    fn focused_address_edit(self) -> Option<HWND> {
        if !self.is_foreground() {
            return None;
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(self.thread, &mut info) } == 0
            || class_name(info.hwndFocus) != "Edit"
            || unsafe {
                IsWindowVisible(info.hwndFocus) == 0 || IsWindowEnabled(info.hwndFocus) == 0
            }
            || unsafe { GetAncestor(info.hwndFocus, GA_ROOT) as usize != self.hwnd }
        {
            return None;
        }
        let mut parent = unsafe { GetParent(info.hwndFocus) };
        while !parent.is_null() && parent as usize != self.hwnd {
            if parent as usize == self.address {
                return Some(info.hwndFocus);
            }
            parent = unsafe { GetParent(parent) };
        }
        None
    }

    /// Called from the foreground thread as a direct consequence of Enter.
    /// The only injected shortcut is Alt+D; subsequent messages target the
    /// verified address edit, never the dialog's Open/Save button.
    pub(crate) fn focus_address(self) -> Result<()> {
        ensure!(self.restore_focus(), "file-continuum-target-unavailable");
        for key in [VK_CONTROL, VK_SHIFT, VK_MENU] {
            ensure!(
                unsafe { GetAsyncKeyState(key as i32) } >= 0,
                "file-continuum-release-modifiers"
            );
        }
        let key = |vk, flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        let keys = [
            key(VK_MENU, 0),
            key(0x44, 0),
            key(0x44, KEYEVENTF_KEYUP),
            key(VK_MENU, KEYEVENTF_KEYUP),
        ];
        // Recheck after inspecting modifiers: a user can change focus while
        // Steward is returning to the original dialog.
        ensure!(self.is_foreground(), "file-continuum-target-unavailable");
        ensure!(
            unsafe {
                SendInput(
                    keys.len() as u32,
                    keys.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            } == keys.len() as u32,
            "file-continuum-target-unavailable"
        );
        Ok(())
    }

    /// All potentially blocking work runs away from GPUI.
    pub(crate) fn navigate(self, path: &Path, cancelled: &AtomicBool) -> Result<()> {
        ensure!(
            path.is_absolute() && path.is_dir(),
            "file-continuum-invalid-directory"
        );
        let mut text: Vec<u16> = path.as_os_str().encode_wide().collect();
        ensure!(!text.contains(&0), "file-continuum-invalid-directory");
        text.push(0);
        for _ in 0..20 {
            ensure!(
                !cancelled.load(Ordering::Acquire),
                "file-continuum-cancelled"
            );
            ensure!(self.is_foreground(), "file-continuum-target-unavailable");
            if let Some(edit) = self.focused_address_edit() {
                let mut result = 0;
                ensure!(
                    unsafe {
                        SendMessageTimeoutW(
                            edit,
                            WM_SETTEXT,
                            0,
                            text.as_ptr() as isize,
                            SMTO_ABORTIFHUNG,
                            250,
                            &mut result,
                        )
                    } != 0
                        && result != 0,
                    "file-continuum-target-unavailable"
                );
                ensure!(
                    !cancelled.load(Ordering::Acquire),
                    "file-continuum-cancelled"
                );
                ensure!(
                    self.focused_address_edit() == Some(edit),
                    "file-continuum-target-unavailable"
                );
                // Send directly to the address edit's subclass. Posting an
                // Enter into the dialog message loop could instead invoke its
                // default Open/Save action if focus changed before dispatch.
                ensure!(
                    unsafe {
                        SendMessageTimeoutW(
                            edit,
                            WM_KEYDOWN,
                            VK_RETURN as usize,
                            0x001c0001,
                            SMTO_ABORTIFHUNG,
                            250,
                            &mut result,
                        )
                    } != 0,
                    "file-continuum-target-unavailable"
                );
                if self.focused_address_edit() == Some(edit) {
                    unsafe {
                        SendMessageTimeoutW(
                            edit,
                            WM_KEYUP,
                            VK_RETURN as usize,
                            0xc01c0001,
                            SMTO_ABORTIFHUNG,
                            250,
                            &mut result,
                        )
                    };
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        bail!("file-continuum-target-unavailable")
    }
}

/// Query only the foreground Explorer's visible view (not an arbitrary tab
/// returned first by ShellWindows). Called on a COM-initialized worker.
pub(crate) fn explorer_directory() -> Option<PathBuf> {
    let foreground = unsafe { GetForegroundWindow() };
    if !matches!(
        class_name(foreground).as_str(),
        "CabinetWClass" | "ExploreWClass"
    ) {
        return None;
    }
    unsafe {
        let shells: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        for index in 0..shells.Count().ok()? {
            let Ok(dispatch) = shells.Item(&VARIANT::from(index)) else {
                continue;
            };
            let Ok(browser) = dispatch.cast::<IWebBrowserApp>() else {
                continue;
            };
            let Ok(browser_hwnd) = browser.HWND() else {
                continue;
            };
            if browser_hwnd.0 as usize != foreground as usize {
                continue;
            }
            let Ok(provider) = browser.cast::<IServiceProvider>() else {
                continue;
            };
            let Ok(shell_browser) = provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser)
            else {
                continue;
            };
            let Ok(view) = shell_browser.QueryActiveShellView() else {
                continue;
            };
            let Ok(view_hwnd) = view.GetWindow() else {
                continue;
            };
            if IsWindowVisible(view_hwnd.0) == 0 || GetAncestor(view_hwnd.0, GA_ROOT) != foreground
            {
                continue;
            }
            // Obtain the path from this exact visible view. The automation
            // Document property can identify a different Explorer tab.
            let Ok(folder_view) = view.cast::<IFolderView>() else {
                continue;
            };
            let Ok(folder) = folder_view.GetFolder::<IPersistFolder2>() else {
                continue;
            };
            let Ok(pidl) = folder.GetCurFolder() else {
                continue;
            };
            let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH);
            CoTaskMemFree(Some(pidl.cast()));
            let Ok(name) = name else { continue };
            let path = PathBuf::from(OsString::from_wide(name.as_wide()));
            CoTaskMemFree(Some(name.0.cast()));
            if GetForegroundWindow() == foreground && path.is_absolute() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ptr::null, ptr::null_mut, sync::Once};
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextW, RegisterClassW,
            WNDCLASSW, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
        },
    };

    // These off-screen windows never activate, and no test injects keys. They
    // exercise the native class/ancestry/identity guards using real HWNDs.
    struct TestDialog(HWND);

    impl TestDialog {
        fn new() -> Self {
            static CLASSES: Once = Once::new();
            CLASSES.call_once(|| {
                for class in [
                    "SHELLDLL_DefView",
                    "Address Band Root",
                    "ComboBoxEx32",
                    "msctls_progress32",
                ] {
                    let class: Vec<_> = class.encode_utf16().chain(Some(0)).collect();
                    let definition = WNDCLASSW {
                        lpfnWndProc: Some(DefWindowProcW),
                        hInstance: unsafe { GetModuleHandleW(null()) },
                        lpszClassName: class.as_ptr(),
                        ..Default::default()
                    };
                    // An application may already have initialized common
                    // controls; CreateWindowExW below checks availability.
                    unsafe { RegisterClassW(&definition) };
                }
            });
            let class: Vec<_> = "#32770".encode_utf16().chain(Some(0)).collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    null(),
                    WS_POPUP | WS_VISIBLE,
                    -32000,
                    -32000,
                    100,
                    100,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                )
            };
            assert!(!hwnd.is_null());
            Self(hwnd)
        }

        fn child(&self, parent: HWND, class: &str, text: &str) -> HWND {
            let class: Vec<_> = class.encode_utf16().chain(Some(0)).collect();
            let text: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    text.as_ptr(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    20,
                    20,
                    parent,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                )
            };
            assert!(!hwnd.is_null());
            hwnd
        }

        fn add_shell_controls(&self) -> HWND {
            self.child(self.0, "SHELLDLL_DefView", "");
            let band = self.child(self.0, "Address Band Root", "");
            let progress = self.child(band, "msctls_progress32", "");
            self.child(progress, "ComboBoxEx32", "")
        }
    }

    impl Drop for TestDialog {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn invalid_or_reused_dialog_never_receives_input() {
        let target = DialogTarget {
            hwnd: 0,
            process: 0,
            thread: 0,
            address: 0,
        };
        assert!(!target.valid());
        assert!(!target.restore_focus());
        assert!(target.focused_address_edit().is_none());
    }

    #[test]
    fn ordinary_dialog_and_unrelated_combo_are_rejected() {
        let dialog = TestDialog::new();
        dialog.child(dialog.0, "Edit", "keep-me.txt");
        assert!(DialogTarget::capture(dialog.0).is_none());
        dialog.child(dialog.0, "SHELLDLL_DefView", "");
        dialog.child(dialog.0, "ComboBoxEx32", "");
        assert!(DialogTarget::capture(dialog.0).is_none());
    }

    #[test]
    fn captured_dialog_rejects_changed_identity_and_destroyed_controls() {
        let dialog = TestDialog::new();
        let address = dialog.add_shell_controls();
        let target = DialogTarget::capture(dialog.0).expect("standard dialog shape");
        assert!(target.valid());
        assert!(!DialogTarget {
            process: target.process.wrapping_add(1),
            ..target
        }
        .valid());
        assert!(!DialogTarget {
            thread: target.thread.wrapping_add(1),
            ..target
        }
        .valid());
        unsafe { DestroyWindow(address) };
        assert!(!target.valid());
    }

    #[test]
    fn dialog_rect_tracks_the_live_window_only() {
        let dialog = TestDialog::new();
        dialog.add_shell_controls();
        let target = DialogTarget::capture(dialog.0).expect("standard dialog shape");
        let mut expected: RECT = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { GetWindowRect(dialog.0, &mut expected) }, 0);
        assert_eq!(
            target.rect(),
            Some(crate::platform::Rect {
                left: expected.left,
                top: expected.top,
                right: expected.right,
                bottom: expected.bottom,
            })
        );
    }

    #[test]
    fn attached_bar_tracks_dialog_moves_width_and_result_height() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        };

        let dialog = TestDialog::new();
        dialog.add_shell_controls();
        let target = DialogTarget::capture(dialog.0).unwrap();
        let bar = TestDialog::new();
        // Keep every window off-screen and non-activating. Exercise the real
        // Win32 resize path with growing/shrinking dialogs and result lists.
        for (x, y, width, height) in [
            (-32000, -32000, 801, 60.0),
            (-31000, -31500, 1103, 400.0),
            (-31500, -31000, 503, 84.0),
        ] {
            assert_ne!(
                unsafe {
                    SetWindowPos(
                        dialog.0,
                        null_mut(),
                        x,
                        y,
                        width,
                        300,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                },
                0
            );
            let anchor = target.rect().unwrap();
            crate::platform::sync_dialog_bounds(bar.0, height, anchor);
            let mut actual: RECT = unsafe { std::mem::zeroed() };
            assert_ne!(unsafe { GetWindowRect(bar.0, &mut actual) }, 0);
            assert_eq!(
                (actual.left, actual.top, actual.right),
                (anchor.left, anchor.bottom, anchor.right)
            );
            assert!(actual.bottom > actual.top);
            // An unchanged tick must preserve the exact same geometry.
            crate::platform::sync_dialog_bounds(bar.0, height, anchor);
            let mut again: RECT = unsafe { std::mem::zeroed() };
            assert_ne!(unsafe { GetWindowRect(bar.0, &mut again) }, 0);
            assert_eq!(
                (again.left, again.top, again.right, again.bottom),
                (actual.left, actual.top, actual.right, actual.bottom)
            );
        }
    }

    #[test]
    fn destroyed_dialog_has_no_rect() {
        let dialog = TestDialog::new();
        let target = DialogTarget {
            hwnd: dialog.0 as usize,
            process: 1,
            thread: 1,
            address: 0,
        };
        assert!(target.rect().is_some());
        drop(dialog);
        assert!(target.rect().is_none());
    }

    #[test]
    fn cancelled_or_unfocused_navigation_leaves_filename_untouched() {
        let dialog = TestDialog::new();
        dialog.add_shell_controls();
        let filename = dialog.child(dialog.0, "Edit", "keep-me.txt");
        let target = DialogTarget::capture(dialog.0).unwrap();
        let path = std::env::temp_dir();
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            target.navigate(&path, &cancelled).unwrap_err().to_string(),
            "file-continuum-cancelled"
        );
        cancelled.store(false, Ordering::Release);
        assert!(!target.is_foreground());
        assert!(target.navigate(&path, &cancelled).is_err());
        let mut text = [0u16; 64];
        let len = unsafe { GetWindowTextW(filename, text.as_mut_ptr(), text.len() as i32) };
        assert_eq!(
            String::from_utf16_lossy(&text[..len as usize]),
            "keep-me.txt"
        );
    }
}
