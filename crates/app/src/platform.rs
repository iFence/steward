//! Platform glue: Win32 window styling, DPI-aware geometry, and the
//! foreground-window / cursor checks the event poll uses to detect that the
//! user clicked away from the launcher. Non-Windows targets get a functional
//! stub so the app still builds (a future milestone adds the native guests).

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "windows")]
mod windows {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::time::Duration;
    use windows_sys::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetMonitorInfoW, GetPixel, MonitorFromPoint, MonitorFromWindow, ReleaseDC,
            SelectObject, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
        UI::WindowsAndMessaging::{
            GetCaretBlinkTime, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect,
            GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SetWindowPos,
            ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
        },
    };

    use crate::config::LAUNCHER_WIDTH;

    /// Declare PerMonitorV2 DPI awareness (Windows 10 1703+). Without this the
    /// OS virtualizes the process at 96 DPI and upscales the window, which
    /// breaks both the launcher's physical size and its dynamic resize. The
    /// call is a no-op (returns false) when the executable already declares
    /// awareness via its manifest, which is fine: the process is aware either
    /// way.
    pub fn set_dpi_awareness() {
        unsafe {
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            use windows_sys::Win32::UI::HiDpi::{
                GetProcessDpiAwareness, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            let result = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // Diagnose mixed-DPI summons (see `dbg_dpi`): if the process failed
            // to reach per-monitor awareness (e.g. a library set it earlier),
            // Windows never delivers WM_DPICHANGED and the bar renders at the
            // system scale on every monitor.
            if std::env::var_os("STEWARD_DBG_DPI").is_some() {
                let mut awareness: i32 = -1;
                let hr = GetProcessDpiAwareness(GetCurrentProcess(), &mut awareness);
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("steward-dpi.log"))
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(
                            f,
                            "startup SetProcessDpiAwarenessContext ok={result} \
                             GetProcessDpiAwareness hr={hr:#x} value={awareness} \
                             (0=unaware 1=system 2=per-monitor)"
                        )
                    });
            }
            let _ = result;
        }
    }

    /// Opt the process into Windows dark mode so native surfaces Steward owns
    /// — the tray context menu, the settings window's title bar — render dark
    /// even when the OS is in light mode.
    ///
    /// Native menus cannot take GPUI's dark theme, so this uses the
    /// undocumented `uxtheme` API (the same technique as win32-darkmode and
    /// tao): set the process-wide preferred app mode to `ForceDark`, then
    /// flush cached menu themes so menus (tray context menu included) are
    /// rebuilt with the dark theme. Must run before any window is created.
    pub fn enable_dark_mode() {
        unsafe {
            let uxtheme = LoadLibraryA(c"uxtheme.dll".as_ptr().cast());
            if uxtheme.is_null() {
                return;
            }

            type SetPreferredAppMode = unsafe extern "system" fn(u32) -> u32;
            type FlushMenuThemes = unsafe extern "system" fn();

            // Undocumented uxtheme ordinals:
            // 135 = SetPreferredAppMode (PreferredAppMode::ForceDark = 2)
            // 136 = FlushMenuThemes
            if let Some(set_preferred_app_mode) =
                std::mem::transmute::<
                    windows_sys::Win32::Foundation::FARPROC,
                    Option<SetPreferredAppMode>,
                >(GetProcAddress(uxtheme, 135usize as *const u8))
            {
                set_preferred_app_mode(2);
            }
            if let Some(flush_menu_themes) =
                std::mem::transmute::<
                    windows_sys::Win32::Foundation::FARPROC,
                    Option<FlushMenuThemes>,
                >(GetProcAddress(uxtheme, 136usize as *const u8))
            {
                flush_menu_themes();
            }
        }
    }

    /// The launcher's native window handle (`None` before the window exists or
    /// after it is closed). Exposed to the event-poll thread so the foreground
    /// watch can query Win32 state without round-tripping through GPUI.
    pub fn hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn is_visible(window: &Window) -> bool {
        hwnd(window).is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 })
    }

    /// HWND-based visibility check, safe to call from the event-poll thread
    /// (only used by the foreground watch).
    pub fn is_hwnd_visible(hwnd: HWND) -> bool {
        unsafe { IsWindowVisible(hwnd) != 0 }
    }

    /// The current foreground (keyboard-focus) window.
    pub fn foreground_hwnd() -> HWND {
        unsafe { GetForegroundWindow() }
    }

    /// Whether the cursor currently sits inside `hwnd`'s window frame. The
    /// foreground watch uses this to keep the launcher up while the user is
    /// still interacting with it (clicking it, dragging it, or composing IME
    /// text) even when it is momentarily not the foreground window.
    pub fn cursor_hits_window(hwnd: HWND) -> bool {
        unsafe {
            let mut cursor: POINT = std::mem::zeroed();
            let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            let got_cursor = GetCursorPos(&mut cursor) != 0;
            let got_rect = GetWindowRect(hwnd, &mut rect) != 0;
            got_cursor
                && got_rect
                && cursor.x >= rect.left
                && cursor.x < rect.right
                && cursor.y >= rect.top
                && cursor.y < rect.bottom
        }
    }

    pub fn hide(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    /// Show the launcher and return the sampled luminance of the backdrop
    /// behind it (`None` when it could not be sampled, e.g. the screen DC
    /// failed). The caller adapts the scrim to that value. `height` is the
    /// intended logical client height in GPUI pixels, identical to what
    /// `resize` was given. Sampling runs *before* `ShowWindow` (so the pixels
    /// read are the backdrop, never the launcher itself), at the rect where
    /// the bar will sit.
    pub fn show(window: &Window, height: f32) -> Option<f32> {
        let hwnd = hwnd(window)?;
        unsafe {
            // Where the bar will sit: monitor under the cursor, sized for that
            // monitor's DPI, centered horizontally and in the upper third of
            // its work area.
            let (x, y, width, height_px, target_dpi) = launcher_rect_for_cursor(hwnd, height);

            let current_monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut cursor: POINT = std::mem::zeroed();
            let target_monitor = if GetCursorPos(&mut cursor) != 0 {
                MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
            } else {
                current_monitor
            };
            let cross = target_monitor != current_monitor;

            // Sample the backdrop *there* while the window is still hidden.
            let backdrop = sample_backdrop_brightness(x, y, width, height_px);

            dbg_dpi("before", hwnd, target_dpi, cross, x, y, width, height_px);
            if cross {
                // Cross-monitor summon. Windows does not re-evaluate a
                // *hidden* window's DPI when it is moved, so a hidden move
                // leaves GPUI's scale factor stale (the content renders at the
                // previous monitor's size — the original multi-monitor bug).
                // Show the window on its current monitor first so its DPI
                // context commits, then move it (now visible) to the target: a
                // visible cross-DPI move reliably delivers WM_DPICHANGED,
                // which updates GPUI's scale factor and resizes the window to
                // the system-suggested rect.
                ShowWindow(hwnd, SW_SHOW);
                SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height_px, SWP_NOACTIVATE);
                dbg_dpi("moved", hwnd, target_dpi, cross, x, y, width, height_px);
            } else {
                SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height_px, SWP_NOACTIVATE);
                ShowWindow(hwnd, SW_SHOW);
            }
            // Self-heal: now that the window is visible, `GetDpiForWindow`
            // reflects its real DPI. Re-apply the exact geometry so any
            // position drift from the system-suggested rect in
            // `WM_DPICHANGED` is corrected and the client area is exactly
            // on-design on this monitor.
            apply_exact(hwnd, height);
            dbg_dpi("after", hwnd, target_dpi, cross, 0, 0, 0, 0);
            force_foreground(hwnd);
            backdrop
        }
    }

    /// Average relative luminance (Rec. 709, 0..1) of the desktop region within
    /// the given physical rect, sampled as a coarse grid of pixels on the
    /// screen DC. The adaptive scrim uses this to raise its opacity over a
    /// bright backdrop (a white document behind the bar), where the frosted
    /// composite would otherwise go light and wash out the white query text.
    /// Returns `None` when the rect or screen DC is unavailable.
    fn sample_backdrop_brightness(left: i32, top: i32, width: i32, height: i32) -> Option<f32> {
        if width <= 0 || height <= 0 {
            return None;
        }
        // GetPixel on the screen DC performs one synchronous display readback
        // per call (measured ~30 ms each on some drivers), which turns a
        // 72-sample grid into a multi-second summon. Instead, copy the launcher
        // region into a memory bitmap with a single BitBlt and sample that
        // in-memory copy: one readback total, and GetPixel on a memory DC reads
        // plain pixels with no round-trip to the display.
        let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
        if screen_dc.is_null() {
            return None;
        }
        let mem_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if mem_dc.is_null() {
            unsafe { ReleaseDC(std::ptr::null_mut(), screen_dc) };
            return None;
        }
        let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, width, height) };
        if bitmap.is_null() {
            unsafe {
                DeleteDC(mem_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return None;
        }
        let previous = unsafe { SelectObject(mem_dc, bitmap) };
        let copied = unsafe { BitBlt(mem_dc, 0, 0, width, height, screen_dc, left, top, SRCCOPY) };
        // 12x6 grid (72 samples): cheap, and averages out text lines or a
        // busy window behind the bar without missing a mostly-white page.
        const COLS: i32 = 12;
        const ROWS: i32 = 6;
        let mut luminance = 0.0f64;
        let mut samples = 0u32;
        if copied != 0 {
            for col in 0..COLS {
                for row in 0..ROWS {
                    let x = left + (width * (2 * col + 1)) / (2 * COLS);
                    let y = top + (height * (2 * row + 1)) / (2 * ROWS);
                    let color = unsafe { GetPixel(mem_dc, x - left, y - top) };
                    // CLR_INVALID (0xFFFFFFFF): the sample fell outside the DC,
                    // e.g. over a monitor not covered by the virtual-screen DC.
                    if color == u32::MAX {
                        continue;
                    }
                    let r = color & 0xFF;
                    let g = (color >> 8) & 0xFF;
                    let b = (color >> 16) & 0xFF;
                    luminance += crate::theme::relative_luminance(r | (g << 8) | (b << 16)) as f64;
                    samples += 1;
                }
            }
        }
        unsafe {
            SelectObject(mem_dc, previous);
            DeleteObject(bitmap);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }
        (samples > 0).then(|| (luminance / samples as f64) as f32)
    }

    /// Associate the default IME context with the launcher so the input
    /// method can compose text (Chinese/Japanese/Korean input).
    pub fn enable_ime(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            use windows_sys::Win32::UI::Input::Ime::{ImmAssociateContextEx, IACE_DEFAULT};
            ImmAssociateContextEx(hwnd, std::ptr::null_mut(), IACE_DEFAULT);
        }
    }

    /// Force the native titlebar to render dark regardless of the OS theme.
    /// GPUI derives `DWMWA_USE_IMMERSIVE_DARK_MODE` from the system appearance
    /// at window creation (light mode => light titlebar); re-applying the
    /// attribute pins the window to the app's dark look, so the settings
    /// window keeps a black titlebar even when Windows is in light mode.
    pub fn force_dark_titlebar(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            use windows_sys::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE,
            };
            let enabled: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &enabled as *const _ as *const _,
                std::mem::size_of_val(&enabled) as u32,
            );
        }
    }

    /// Convert a desired logical client size into the physical window size
    /// including the native non-client frame. The launcher is borderless, but
    /// Windows still frames WS_POPUP windows with a few device pixels; if they
    /// are not added, the client area ends up shorter than requested and the
    /// flex column clips the drop-down's last row.
    ///
    /// `dpi` is the scale to size for explicitly: `resize` sizes in place
    /// (the window's own DPI), while `launcher_rect_for_cursor` sizes for the
    /// *target* monitor before the window moves there, so a cross-DPI move
    /// lands at the right physical size.
    unsafe fn client_to_window_px(hwnd: HWND, width: f32, height: f32, dpi: u32) -> (i32, i32) {
        let dpi = dpi.max(96);
        let scale = dpi as f32 / 96.0;
        let mut window_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        let mut client_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut window_rect);
        GetClientRect(hwnd, &mut client_rect);
        let border_x = (window_rect.right - window_rect.left) - client_rect.right;
        let border_y = (window_rect.bottom - window_rect.top) - client_rect.bottom;
        (
            (width * scale).round() as i32 + border_x,
            (height * scale).round() as i32 + border_y,
        )
    }

    /// Resize the launcher window so its client area is `LAUNCHER_WIDTH` x
    /// `height` logical pixels, keeping the current top-left corner so the
    /// drop-down grows downward. `height` is the same DPI-aware unit GPUI's
    /// layout uses; physical pixels are derived from the window's own DPI.
    pub fn resize(window: &Window, height: f32) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            let (width_px, height_px) =
                client_to_window_px(hwnd, LAUNCHER_WIDTH, height, GetDpiForWindow(hwnd));
            let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                width_px,
                height_px,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Full on/off period of the caret blink. Windows exposes the OS caret
    /// blink time (the on-phase), so a full blink cycle is twice that. If the
    /// OS reports blink disabled (0), fall back to the standard 1.06 s cycle.
    pub fn caret_blink_period() -> Duration {
        let on_ms = unsafe { GetCaretBlinkTime() };
        if on_ms == 0 {
            Duration::from_millis(1060)
        } else {
            Duration::from_millis(u64::from(on_ms) * 2)
        }
    }

    /// Compute where the launcher should sit for the monitor under the cursor:
    /// sized for `height` logical px at that monitor's DPI (borders included),
    /// centered horizontally and in the upper third of its work area. Returns
    /// `(x, y, width_px, height_px, dpi)`.
    ///
    /// The window is created hidden on the primary display, so
    /// `MonitorFromWindow` would always report that monitor; pick the one
    /// under the cursor instead (falling back to the window's own monitor when
    /// the cursor query fails). Likewise `GetDpiForWindow` is read only as a
    /// fallback: at this point the window still sits on the monitor it was
    /// last shown on, so on mixed-DPI setups it would size the bar for the
    /// wrong scale — read the *target* monitor's DPI instead.
    unsafe fn launcher_rect_for_cursor(hwnd: HWND, height: f32) -> (i32, i32, i32, i32, u32) {
        let mut cursor: POINT = std::mem::zeroed();
        let monitor = if GetCursorPos(&mut cursor) != 0 {
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
        };
        let mut dpi_x: u32 = 0;
        let mut dpi_y: u32 = 0;
        let dpi = if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) >= 0
            && dpi_x > 0
            && dpi_y > 0
        {
            dpi_x.max(dpi_y)
        } else {
            GetDpiForWindow(hwnd)
        }
        .max(96);
        let (width, height_px) = client_to_window_px(hwnd, LAUNCHER_WIDTH, height, dpi);

        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - width) / 2;
        // Horizontally centered; vertically in the upper third of the screen.
        let y = work.top + ((work.bottom - work.top) - height_px) / 3;
        (x, y, width, height_px, dpi)
    }

    /// Re-apply the launcher's exact geometry on the monitor it currently sits
    /// on, using the window's own DPI. Called after `ShowWindow`, when
    /// `GetDpiForWindow` is truthful: it self-heals any position drift or size
    /// rounding introduced by the system-suggested rect in `WM_DPICHANGED`.
    unsafe fn apply_exact(hwnd: HWND, height: f32) {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let (width, height_px) = client_to_window_px(hwnd, LAUNCHER_WIDTH, height, dpi);

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - width) / 2;
        let y = work.top + ((work.bottom - work.top) - height_px) / 3;

        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height_px, SWP_NOACTIVATE);
    }

    /// Append a one-line snapshot of the summon-time DPI state to
    /// `%TEMP%\steward-dpi.log` while the `STEWARD_DBG_DPI` env var is set (a
    /// no-op otherwise). Used to diagnose cross-monitor sizing on mixed-DPI
    /// setups. `dpi`/`cross` are the caller's expected target dpi and
    /// cross-monitor flag; `set_x`..`set_h` the rect about to be applied.
    #[allow(clippy::too_many_arguments)]
    fn dbg_dpi(
        tag: &str,
        hwnd: HWND,
        dpi: u32,
        cross: bool,
        set_x: i32,
        set_y: i32,
        set_w: i32,
        set_h: i32,
    ) {
        if std::env::var_os("STEWARD_DBG_DPI").is_none() {
            return;
        }
        unsafe {
            let mut cursor: POINT = std::mem::zeroed();
            let cursor_ok = GetCursorPos(&mut cursor) != 0;
            let mut window_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            let rect_ok = GetWindowRect(hwnd, &mut window_rect) != 0;
            let mut client_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            let client_ok = GetClientRect(hwnd, &mut client_rect) != 0;
            let line = format!(
                "{tag} cursor=({},{},ok={}) target_dpi={} cross={} win_dpi={} window=({},{},{},{},ok={}) client=({},{},ok={}) set=({},{},{},{})\n",
                cursor.x,
                cursor.y,
                cursor_ok,
                dpi,
                cross,
                GetDpiForWindow(hwnd),
                window_rect.left,
                window_rect.top,
                window_rect.right,
                window_rect.bottom,
                rect_ok,
                client_rect.right,
                client_rect.bottom,
                client_ok,
                set_x,
                set_y,
                set_w,
                set_h,
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("steward-dpi.log"))
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(line.as_bytes())
                });
        }
    }

    /// Windows restricts `SetForegroundWindow`; attaching the input queues of
    /// the involved threads is the standard workaround for launcher-style apps.
    unsafe fn force_foreground(hwnd: HWND) {
        let current_thread = GetCurrentThreadId();
        let target_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());

        if current_thread != target_thread {
            AttachThreadInput(current_thread, target_thread, 1);
        }
        if current_thread != foreground_thread && foreground_thread != 0 {
            AttachThreadInput(current_thread, foreground_thread, 1);
        }

        SetForegroundWindow(hwnd);

        if current_thread != target_thread {
            AttachThreadInput(current_thread, target_thread, 0);
        }
        if current_thread != foreground_thread && foreground_thread != 0 {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub::*;

#[cfg(not(target_os = "windows"))]
mod stub {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use gpui::Window;

    static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

    pub fn is_visible(_window: &Window) -> bool {
        WINDOW_VISIBLE.load(Ordering::Relaxed)
    }

    pub fn hide(_window: &Window) {
        WINDOW_VISIBLE.store(false, Ordering::Relaxed);
    }

    pub fn show(_window: &Window, _height: f32) -> Option<f32> {
        WINDOW_VISIBLE.store(true, Ordering::Relaxed);
        None
    }

    /// Resizing is a Windows-specific launcher behavior for now.
    pub fn resize(_window: &Window, _height: f32) {}

    /// Native titlebar dark-mode forcing is Windows-only; other platforms
    /// already follow the app theme.
    pub fn force_dark_titlebar(_window: &Window) {}

    pub fn caret_blink_period() -> Duration {
        Duration::from_millis(1060)
    }
}
