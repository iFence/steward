//! Steward desktop entry point.
//!
//! Startup is silent: the app registers a system tray icon (Windows/macOS)
//! and the global hotkey `Ctrl+Alt+Space`, but opens no window. The launcher
//! bar — a wide, short, borderless popup centered on the primary display —
//! is summoned on demand via the hotkey or the tray icon, and hidden again
//! with `Esc` (or the hotkey), or automatically when another application
//! takes activation (clicking away).
//!
//! GPUI does not provide a system-level hotkey API, so registration is
//! delegated to the `global-hotkey` crate and events are bridged into the
//! GPUI main loop through channels polled by a foreground task. Tray and
//! tray-menu events ride the same loop.
//!
//! Window visibility on Windows is driven through the native HWND
//! (`ShowWindow(SW_HIDE/SW_SHOW)`), because `App::hide` is a no-op on the
//! Windows backend of the pinned GPUI revision. Other platforms fall back to
//! `App::hide` / `App::activate` (see docs/architecture.md, M4 will polish
//! non-Windows behavior).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    cell::RefCell, collections::HashMap, ops::Range, process::Command, rc::Rc, sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gpui::{
    actions, div, point, prelude::*, px, rgb, size, Anchor, Animation, AnimationExt, AnyElement,
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Div, Element, ElementId,
    ElementInputHandler, EntityInputHandler, FocusHandle, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, KeyBinding, KeyDownEvent, LayoutId, Pixels, QuitMode,
    SharedString, Subscription, TitlebarOptions, UTF16Selection, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions,
};
use gpui_component::{
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    theme::{Theme, ThemeTokens},
    v_flex, ActiveTheme, Icon, IconName, Root, StyledExt,
};
use gpui_platform::application;
use steward_core_engine::Engine;
use steward_ui_components::{ResultList, ResultListDelegate};

mod i18n;

#[cfg(target_os = "windows")]
mod app_icons;

#[cfg(not(target_os = "windows"))]
mod app_icons {
    /// Icon extraction is Windows-only for M1; other platforms render rows
    /// without icons until a scanner provides them.
    pub fn app_icon_image(_path: &std::path::Path) -> Option<std::sync::Arc<gpui::Image>> {
        None
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon as TrayIcon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

actions!(steward, [HideWindow]);

/// The launcher bar is deliberately long and short: wide enough to hold a
/// search box plus quick-launch chips, short enough to sit unobtrusively in
/// the middle of the screen. These are logical (CSS) pixels: on a 200% scaled
/// display GPUI renders them at 2× physical pixels, like any other app.
const LAUNCHER_WIDTH: f32 = 760.0;
const LAUNCHER_HEIGHT: f32 = 60.0;
/// Width of the non-interactive margin around the input box. This margin is
/// the window's drag handle; the input box itself is not draggable.
const LAUNCHER_MARGIN: f32 = 4.0;

/// Fixed row height of a launcher result. Must match `results_list.rs` so the
/// window resize stays in sync with the rendered list.
const RESULT_ROW_HEIGHT: f32 = 48.0;
/// Maximum number of results shown before the drop-down scrolls.
const MAX_RESULT_ROWS: usize = 8;

const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";

/// Whether hiding the launcher destroys its window — reclaiming the
/// per-window renderer, swapchain and GPU atlas — instead of keeping it
/// allocated. Measured on the Windows dev machine (docs/benchmarks.md):
/// closing the window reclaims almost nothing — the DirectX devices and
/// DirectWrite text system are platform-level and stay allocated for the GPUI
/// session — while re-summoning then costs ~150 ms. Keeping the window hidden
/// re-summons in ~10-30 ms at the same memory, so hide is the default.
const CLOSE_ON_HIDE: bool = false;

/// The launcher's accent color when no theme color is configured (Catppuccin
/// Mocha blue). Doubles as the default selection/caret color.
const DEFAULT_ACCENT: u32 = 0x89b4fa;
/// Surface colors shared by the launcher bar and the settings window. The
/// settings window renders on the gpui-component dark theme's near-black
/// background by default, which reads much darker than the launcher's bar.
const PANEL_BACKGROUND: u32 = 0x232332;
const PANEL_BACKGROUND_ALT: u32 = 0x2a2a3c;
const PANEL_BORDER: u32 = 0x3a3a4c;
/// Storage key for the persisted theme color (a `#rrggbb` hex string).
const THEME_COLOR_SETTING: &str = "theme_color";
/// Storage key for the persisted language code (e.g. `zh`, `en`).
const LANGUAGE_SETTING: &str = "language";
/// Preset accent colors offered by the settings page.
const ACCENT_PRESETS: [u32; 4] = [
    0x89b4fa, // sea blue (default)
    0x94e2d5, // jade
    0xf38ba8, // rose
    0xf9e2af, // amber
];
/// Languages offered in the settings dropdown: (Fluent resource code, native
/// display name). Native names are correct in every language, so they need no
/// translation.
const SUPPORTED_LANGUAGES: [(&str, &str); 7] = [
    ("zh", "中文"),
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

/// Per-user autostart registry key (HKCU\...\Run). Values run at logon before
/// the shell starts; no admin rights are needed to write the current user's
/// key, and it travels with the user profile.
#[cfg(target_os = "windows")]
const AUTOSTART_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const AUTOSTART_VALUE_NAME: &str = "Steward";

/// Height of the results drop-down for `count` visible rows, capped at
/// `MAX_RESULT_ROWS` so the window stops growing once the list scrolls.
fn result_height(count: usize) -> f32 {
    RESULT_ROW_HEIGHT * count.min(MAX_RESULT_ROWS) as f32
}

/// Total launcher window height for a given number of visible result rows:
/// the input bar plus the top/bottom drag margins plus the (possibly
/// scroll-capped) drop-down. The margins are part of the window, so they must
/// be counted or the flex column squeezes the drop-down's last row.
fn launcher_height(result_count: usize) -> f32 {
    LAUNCHER_HEIGHT + 2.0 * LAUNCHER_MARGIN + result_height(result_count)
}

/// Parse a `#rrggbb` hex string into an RGB integer, or `None` when malformed.
fn parse_hex_color(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

/// Apply the Steward palette (surface colors matching the launcher bar) and
/// the given accent color to the global gpui-component theme, then rebuild the
/// derived semantic tokens and the Base-layer theme (scrollbars, resize
/// handles). Call once at startup after `init_components`, and again whenever
/// the user picks a new theme color in the settings window.
fn apply_steward_theme(cx: &mut App, accent: u32) {
    let background = Hsla::from(rgb(PANEL_BACKGROUND));
    let background_alt = Hsla::from(rgb(PANEL_BACKGROUND_ALT));
    let border = Hsla::from(rgb(PANEL_BORDER));
    let foreground = Hsla::from(rgb(0xcdd6f4));
    let muted_foreground = Hsla::from(rgb(0x6c7086));
    let accent = Hsla::from(rgb(accent));

    let theme = Theme::global_mut(cx);
    // Surfaces: the settings window's background, side bar, groups and popups
    // now share the launcher's color instead of the near-black dark default.
    theme.background = background;
    theme.foreground = foreground;
    theme.popover = background;
    theme.popover_foreground = foreground;
    theme.border = border;
    theme.input = border;
    theme.muted = background_alt;
    theme.muted_foreground = muted_foreground;
    theme.secondary = background_alt;
    theme.secondary_foreground = foreground;
    theme.secondary_hover = Hsla::from(rgb(0x313244));
    theme.accent = background_alt;
    theme.accent_foreground = foreground;
    theme.colors.list = background;
    theme.list_hover = background_alt;
    theme.group_box = background;
    theme.group_box_foreground = foreground;
    theme.sidebar = background;
    theme.sidebar_border = border;
    theme.sidebar_foreground = foreground;
    theme.sidebar_accent = background_alt;
    theme.tiles = background;
    theme.table = background;
    theme.title_bar = background;
    theme.title_bar_border = border;
    theme.window_border = border;
    theme.tab_bar = background;
    theme.tab = background;
    theme.tab_active = background_alt;
    // Accent: selection, caret, focus ring and primary controls follow the
    // chosen theme color.
    theme.primary = accent;
    theme.primary_hover = accent.opacity(0.85);
    theme.primary_active = accent.opacity(0.75);
    theme.primary_foreground = Hsla::from(rgb(0x11111b));
    theme.button_primary = accent;
    theme.button_primary_hover = accent.opacity(0.85);
    theme.button_primary_active = accent.opacity(0.75);
    theme.button_primary_foreground = Hsla::from(rgb(0x11111b));
    theme.caret = accent;
    theme.selection = accent.opacity(0.35);
    theme.ring = accent;
    theme.list_active = accent.opacity(0.2);
    theme.list_active_border = accent.opacity(0.6);
    theme.drop_target = accent.opacity(0.2);

    // Re-derive the full legacy token set (sidebar included — the sidebar
    // widget reads `tokens.sidebar`, not `colors.sidebar`) from the mutated
    // colors, then push the Base-layer theme (scrollbars, resize handles).
    theme.tokens = ThemeTokens::from(theme.colors);
    Theme::sync_base(cx);
    cx.refresh_windows();
}

/// Spawn the application at `path`, detaching from the launcher process so
/// both keep running independently.
fn launch(path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let text = path.to_string_lossy();
        // `.lnk` shortcuts (shell namespace items such as Control Panel) and
        // `shell:` UWP aliases can't be spawned via CreateProcess; let the
        // shell resolve them.
        if text.starts_with("shell:") || text.to_ascii_lowercase().ends_with(".lnk") {
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
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
            return Ok(());
        }
        // `Command::new` resolves the path; keep the child detached by not
        // holding a handle to it.
        let _child = Command::new(path).spawn().context("launch application")?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows launching is stubbed for M1.
        anyhow::bail!("launching is not yet implemented on this platform");
    }
    Ok(())
}

struct StewardApp {
    focus_handle: FocusHandle,
    input: SearchInput,
    i18n: Rc<i18n::Localization>,
    /// Shared search index, rebuilt at startup from a scan / cache.
    engine: Rc<RefCell<Engine>>,
    /// SQLite cache plus usage-frequency tracking.
    storage: Rc<RefCell<steward_storage::Storage>>,
    /// Current result rows, mirrored for the `on_confirm` callback to resolve
    /// the selected index into an app path.
    last_results: Rc<RefCell<Vec<steward_core_engine::AppEntry>>>,
    results: ResultList,
    /// Result count is published here so the tray/hotkey path can size the
    /// window when it is summoned.
    state: Rc<RefCell<LauncherState>>,
    /// Keeps the window-activation observer alive for the view's lifetime.
    _activation_subscription: Subscription,
}

struct SearchInput {
    query: String,
    /// Cursor position measured in characters.
    cursor: usize,
    /// Active IME composition range, measured in characters. `None` when no
    /// input method is composing text.
    marked: Option<Range<usize>>,
}

/// Shared launcher state used by the foreground event loop: the (possibly
/// closed) window handle plus the focus handle that must be re-focused every
/// time the bar is summoned, and the current result count so the drop-down
/// height can be computed at show time.
struct LauncherState {
    window: Option<AnyWindowHandle>,
    settings_window: Option<AnyWindowHandle>,
    /// Created together with GPUI (a `FocusHandle` can only be allocated from
    /// an application context); `None` before the first summon.
    focus: Option<FocusHandle>,
    result_count: usize,
    /// Shared SQLite storage (app index cache, usage, persisted settings).
    storage: Rc<RefCell<steward_storage::Storage>>,
    /// Shared search index (entries + pre-built pinyin haystacks), rebuilt
    /// from the SQLite cache at boot and refreshed in the background. Lives
    /// outside the window so recreating the launcher never re-scans.
    engine: Rc<RefCell<Engine>>,
    /// Extracted app icons (PNG-encoded `gpui::Image`), keyed by app path.
    /// `None` entries mark paths whose icon could not be extracted. Shared so
    /// a recreated window keeps its icons.
    icon_cache: RefCell<HashMap<std::path::PathBuf, Option<Arc<gpui::Image>>>>,
    /// Pending background scan results, drained by the GPUI foreground poll
    /// task.
    scan_rx: RefCell<Option<crossbeam_channel::Receiver<Vec<steward_core_engine::AppEntry>>>>,
    /// Logical launcher height last requested from a resize, so `search` can
    /// skip redundant resize calls when the result-count-driven height has not
    /// changed (e.g. every IME composition update).
    last_applied_height: f32,
}

impl LauncherState {
    /// Total launcher window height for the current result count: the input
    /// bar plus the result drop-down.
    fn height(&self) -> f32 {
        launcher_height(self.result_count)
    }

    /// Seed the search index from the SQLite cache and, when the cache is
    /// missing or stale, start a background scan whose results are applied by
    /// [`Self::apply_scan_results`]. Never blocks the caller: the scan runs
    /// on a worker thread and both event loops drain the result channel.
    fn ensure_app_index(&self) {
        let cached = self.storage.borrow().cached_apps().unwrap_or_default();
        let has_cache = !cached.is_empty();
        if has_cache {
            self.engine.borrow_mut().set_entries(cached);
        }
        let fresh = self.storage.borrow().is_cache_fresh() && has_cache;
        if fresh || self.scan_rx.borrow().is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let apps = steward_core_engine::platform_scanner().scan();
            let _ = tx.send(apps);
        });
        *self.scan_rx.borrow_mut() = Some(rx);
    }

    /// Apply a finished background scan: persist the cache and rebuild the
    /// index. Runs on the main thread from the GPUI foreground poll task.
    fn apply_scan_results(&self) {
        let Some(rx) = self.scan_rx.borrow().clone() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(apps) if !apps.is_empty() => {
                    if let Err(error) = self.storage.borrow_mut().mark_seen(&apps) {
                        eprintln!("failed to persist scanned apps: {error:#}");
                    }
                    self.engine.borrow_mut().set_entries(apps);
                }
                // Empty scan (e.g. unsupported platform): keep the cache.
                Ok(_) => {}
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    *self.scan_rx.borrow_mut() = None;
                    break;
                }
            }
        }
    }
}

impl SearchInput {
    fn char_count(&self) -> usize {
        self.query.chars().count()
    }

    fn byte_index(&self) -> usize {
        self.query
            .char_indices()
            .nth(self.cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len())
    }

    fn insert_char(&mut self, ch: char) {
        self.query.insert(self.byte_index(), ch);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let byte = self
                .query
                .char_indices()
                .nth(self.cursor - 1)
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.query.remove(byte);
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.char_count() {
            let byte = self.byte_index();
            self.query.remove(byte);
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        self.cursor = (self.cursor as i32 + delta).clamp(0, self.char_count() as i32) as usize;
    }

    fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.char_count());
    }

    fn utf16_len(&self) -> usize {
        self.query.encode_utf16().count()
    }

    /// Byte offset of the character at `char_index`.
    fn byte_at_char(&self, char_index: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len())
    }

    /// Replace the given UTF-16 range with `text` (or the active composition,
    /// or insert at the caret when the platform passes no range) and place the
    /// caret after it. Clears any active composition.
    fn replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) {
        match range.and_then(|r| self.utf16_to_chars(r)) {
            Some(range) => {
                let start = self.byte_at_char(range.start);
                let end = self.byte_at_char(range.end);
                self.query.replace_range(start..end, text);
                self.cursor = range.start + text.chars().count();
            }
            None => {
                // The Windows IME passes `None` for the document; replace the
                // active composition when present, else insert at the caret.
                if let Some(marked) = self.marked.clone() {
                    let start = self.byte_at_char(marked.start);
                    let end = self.byte_at_char(marked.end);
                    self.query.replace_range(start..end, text);
                    self.cursor = marked.start + text.chars().count();
                } else {
                    let byte = self.byte_at_char(self.cursor);
                    self.query.insert_str(byte, text);
                    self.cursor += text.chars().count();
                }
            }
        }
        self.marked = None;
    }

    /// Replace text and mark the replacement as an active composition.
    fn replace_and_mark_utf16(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
    ) {
        self.replace_utf16(range, text);
        let text_len_chars = text.chars().count();
        let end = self.cursor;
        let start = end.saturating_sub(text_len_chars);
        self.marked = Some(start..end);
        if let Some(sel) = new_selected {
            let utf16_before = self.char_to_utf16(start);
            let utf16_sel = (utf16_before + sel.start)..(utf16_before + sel.end);
            if let Some(chars) = self.utf16_to_chars(utf16_sel) {
                self.cursor = chars.start;
            }
        }
    }

    /// UTF-16 index of a character index.
    fn char_to_utf16(&self, char_index: usize) -> usize {
        self.query
            .chars()
            .take(char_index)
            .map(|c| c.len_utf16())
            .sum()
    }

    /// Character range corresponding to a UTF-16 range.
    fn utf16_to_chars(&self, utf16: Range<usize>) -> Option<Range<usize>> {
        let mut units = 0;
        let mut start = None;
        for (chars, c) in self.query.chars().enumerate() {
            let len = c.len_utf16();
            if start.is_none() && units + len > utf16.start {
                start = Some(chars);
            }
            units += len;
            if units >= utf16.end {
                return start.map(|s| s..chars + 1);
            }
        }
        None
    }
}

/// GPUI's text-input interface. The launcher's query is a single-line
/// document; the Windows platform routes IME composition (Chinese/Japanese/
/// Korean input) and `WM_CHAR` text through these callbacks.
impl EntityInputHandler for StewardApp {
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.input.utf16_len())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.input.char_to_utf16(self.input.cursor);
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.input
            .marked
            .as_ref()
            .map(|range| self.input.char_to_utf16(range.start)..self.input.char_to_utf16(range.end))
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let chars = self.input.utf16_to_chars(range_utf16.clone())?;
        *adjusted_range =
            Some(self.input.char_to_utf16(chars.start)..self.input.char_to_utf16(chars.end));
        Some(
            self.input.query
                [self.input.byte_at_char(chars.start)..self.input.byte_at_char(chars.end)]
                .to_string(),
        )
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.replace_utf16(range, text);
        self.search(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input
            .replace_and_mark_utf16(range, new_text, new_selected_range);
        self.search(window, cx);
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.input.marked = None;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Approximate the caret position for the IME candidate window: input
        // padding plus an estimated glyph width per character.
        let chars = self.input.utf16_to_chars(range_utf16).unwrap_or(0..0);
        let x = 12.0 + 9.0 * chars.start as f32;
        Some(Bounds::new(
            point(element_bounds.origin.x + px(x), element_bounds.origin.y),
            size(px(2.0), element_bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }
}

/// Wraps the launcher's root element so the window can register it as the
/// active text-input handler during paint — this is what makes IME composition
/// (and `WM_CHAR`) reach the launcher's query.
struct LauncherInputElement {
    child: AnyElement,
    focus_handle: FocusHandle,
    view: gpui::Entity<StewardApp>,
    input_bounds: Bounds<Pixels>,
}

impl Element for LauncherInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // The input area is the top bar of the window (the drop-down below is
        // results, not text input).
        self.input_bounds =
            Bounds::new(bounds.origin, size(bounds.size.width, px(LAUNCHER_HEIGHT)));
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(self.input_bounds, self.view.clone()),
            cx,
        );
    }
}

impl IntoElement for LauncherInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Render for StewardApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI's Windows platform disables the IME context from its WM_PAINT
        // path whenever the input handler is momentarily unavailable (taken
        // during the draw). Re-associating every frame keeps composition input
        // available to the launcher.
        #[cfg(target_os = "windows")]
        platform::enable_ime(window);

        let result_count = self.results.visible_count(cx);
        let primary = cx.theme().primary;
        let root = div()
            .track_focus(&self.focus_handle)
            .on_action(|_: &HideWindow, window, cx| hide_window(window, cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            // Semi-transparent so the window-level blur shows through.
            .bg(rgb(0x232332).opacity(0.62))
            .text_lg()
            .text_color(rgb(0xcdd6f4))
            .child(drag_strip().h(px(LAUNCHER_MARGIN)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(drag_strip().w(px(LAUNCHER_MARGIN)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .items_center()
                            .px_3()
                            .cursor_text()
                            .child(
                                if self.input.query.is_empty() && self.input.marked.is_none() {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .child(cursor(primary))
                                        .child(
                                            div()
                                                .text_color(rgb(0x6c7086))
                                                .child(self.i18n.translate("search-placeholder")),
                                        )
                                } else {
                                    self.render_query_text(primary)
                                },
                            ),
                    )
                    .child(drag_strip().w(px(LAUNCHER_MARGIN))),
            )
            .when(result_count > 0, |this| {
                // A subtle light-gray hairline separates the search box from
                // the results list; it only shows while the drop-down is open.
                let drop_height = result_height(result_count);
                this.child(div().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.12)))
                    // Pin the drop-down to exactly its result height so it never
                    // grows into the input bar, and inset it by the same margin as
                    // the drag strips so rows align with the bar content.
                    .child(
                        div()
                            .h(px(drop_height))
                            .mx(px(LAUNCHER_MARGIN))
                            .child(self.results.render(drop_height, cx)),
                    )
            })
            .child(drag_strip().h(px(LAUNCHER_MARGIN)));

        LauncherInputElement {
            child: root.into_any_element(),
            focus_handle: self.focus_handle.clone(),
            view: cx.entity(),
            input_bounds: Bounds::default(),
        }
    }
}

impl StewardApp {
    /// Render the query text: leading text, the active IME composition
    /// (underlined), the caret and the trailing text.
    fn render_query_text(&self, primary: Hsla) -> Div {
        let query = &self.input.query;
        let (pre, marked, post) = match &self.input.marked {
            Some(range) => {
                let start = self.input.byte_at_char(range.start);
                let end = self.input.byte_at_char(range.end);
                (&query[..start], Some(&query[start..end]), &query[end..])
            }
            None => {
                let start = self.input.byte_at_char(self.input.cursor);
                (&query[..start], None, &query[start..])
            }
        };

        let mut children: Vec<AnyElement> = Vec::new();
        if !pre.is_empty() {
            children.push(div().child(pre.to_string()).into_any_element());
        }
        if let Some(marked) = marked {
            children.push(
                div()
                    .underline()
                    .child(marked.to_string())
                    .into_any_element(),
            );
        }
        children.push(cursor(primary).into_any_element());
        if !post.is_empty() {
            children.push(div().child(post.to_string()).into_any_element());
        }

        div().flex().flex_row().items_center().children(children)
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        // While an IME composition is active the input method owns the editing
        // keys (including Enter/Escape/arrows used to pick candidates); leave
        // the query alone and let the platform drive the composition through
        // `EntityInputHandler` callbacks.
        if self.input.marked.is_some() {
            cx.stop_propagation();
            return;
        }

        // Insert the typed character (key_char also carries the shifted or
        // AltGr variant). Skip when ctrl/alt/win are held, e.g. shortcuts.
        if !modifiers.control && !modifiers.alt && !modifiers.platform {
            if let Some(ch) = keystroke.key_char.as_deref().and_then(|s| s.chars().next()) {
                if !ch.is_control() {
                    self.input.insert_char(ch);
                    self.search(window, cx);
                    cx.stop_propagation();
                    return;
                }
            }
        }

        match keystroke.key.as_str() {
            "space" => {
                self.input.insert_char(' ');
                self.search(window, cx);
                cx.stop_propagation();
            }
            "backspace" => {
                self.input.backspace();
                self.search(window, cx);
                cx.stop_propagation();
            }
            "delete" => {
                self.input.delete();
                self.search(window, cx);
                cx.stop_propagation();
            }
            // Up / Down move the result selection; Enter confirms (launches).
            "up" => {
                self.results.select_relative(-1, window, cx);
                cx.stop_propagation();
            }
            "down" => {
                self.results.select_relative(1, window, cx);
                cx.stop_propagation();
            }
            "enter" => {
                // Confirm fires the delegate's `on_confirm`, which launches the
                // selected app and records its usage; then reset and hide.
                if self.results.confirm_selected(window, cx) {
                    self.after_confirm(window, cx);
                }
                cx.stop_propagation();
            }
            "left" => {
                self.input.move_cursor(-1);
                cx.notify();
                cx.stop_propagation();
            }
            "right" => {
                self.input.move_cursor(1);
                cx.notify();
                cx.stop_propagation();
            }
            "home" => {
                self.input.set_cursor(0);
                cx.notify();
                cx.stop_propagation();
            }
            "end" => {
                self.input.set_cursor(self.input.char_count());
                cx.notify();
                cx.stop_propagation();
            }
            // Hide directly at the key level (the keybinding is a fallback):
            // this is more robust than relying on action dispatch when the
            // window just went through a drag or was re-activated.
            "escape" => {
                hide_window(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Run the current input through the engine and refresh the results list,
    /// mirroring the results for the confirm callback to resolve against, then
    /// resize the window to fit the new drop-down height.
    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.input.query.clone();
        let items = self
            .engine
            .borrow()
            .query(&query, &|path| self.storage.borrow().frequency_str(path));

        // Resolve an icon per visible result, reusing the cache so only new
        // paths pay the (cheap) Win32 extraction cost. Extraction happens on
        // the UI thread but is bounded to the visible rows and cached; rows
        // below the fold render without an icon for now. The cache lives in
        // the shared launcher state so a recreated window keeps its icons.
        let icons = {
            let state = self.state.borrow();
            items
                .iter()
                .take(MAX_RESULT_ROWS)
                .map(|app| {
                    // `let` ends the temporary borrow before `unwrap_or_else`,
                    // so the miss path can `borrow_mut` again.
                    let cached = state.icon_cache.borrow_mut().get(&app.path).cloned();
                    cached.unwrap_or_else(|| {
                        let icon = app_icons::app_icon_image(&app.path);
                        state
                            .icon_cache
                            .borrow_mut()
                            .insert(app.path.clone(), icon.clone());
                        icon
                    })
                })
                .collect::<Vec<_>>()
        };
        let missing = items.len().saturating_sub(icons.len());
        let icons = icons
            .into_iter()
            .chain(std::iter::repeat_n(None, missing))
            .collect::<Vec<_>>();

        *self.last_results.borrow_mut() = items.clone();
        self.results.set_results(items, icons, cx);

        let count = self.results.visible_count(cx);
        let height = launcher_height(count);
        let mut state = self.state.borrow_mut();
        state.result_count = count;
        // Resize through GPUI's own window API, which runs the native
        // SetWindowPos asynchronously on the foreground executor. A
        // synchronous platform-layer resize while the launcher is visible
        // desyncs the pinned GPUI Windows renderer's viewport: the drop-down
        // area renders a cleared white strip and content is offset or missing
        // (regression documented in docs/architecture.md). The async path
        // applies the size at a clean point in the event loop, and the backend
        // compensates the native frame via `border_offset`, so the client area
        // still lands exactly on the requested logical size. Skip when the
        // height did not change so IME composition updates don't spam
        // redundant resizes.
        if (height - state.last_applied_height).abs() > 0.5 {
            state.last_applied_height = height;
            window.resize(size(px(LAUNCHER_WIDTH), px(height)));
        }
        cx.notify();
    }

    /// Reset the launcher to its idle (bar-only) state and hide it. Called
    /// after the delegate's confirm callback has launched the selected app.
    fn after_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.query.clear();
        self.input.cursor = 0;
        self.input.marked = None;
        // Re-run the empty query so the next summon opens with the most-used
        // applications instead of an empty bar (the recents seed in
        // `open_launcher_window` only runs once, at window creation).
        self.search(window, cx);
        hide_window(window, cx);
        cx.notify();
    }
}

/// A blinking text cursor rendered as a thin vertical bar, using the same
/// on/off cadence as the OS caret.
fn cursor(primary: Hsla) -> impl IntoElement {
    div().w(px(2.0)).h(px(18.0)).bg(primary).with_animation(
        "cursor-blink",
        Animation::new(platform::caret_blink_period()).repeat_synced(),
        |this, delta| this.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
    )
}

/// A transparent strip around the launcher content that acts as the window's
/// drag handle. Only these margins start a drag; the input box in the middle
/// stays interactive (text cursor) instead of dragging the window.
fn drag_strip() -> Div {
    div().window_control_area(WindowControlArea::Drag)
}

fn main() {
    // Opt into per-monitor DPI awareness before any window exists so GPUI and
    // the launcher size calculations see the real display scale (e.g. 2.0 at
    // 200%) instead of the 96-DPI virtualization the OS applies otherwise.
    #[cfg(target_os = "windows")]
    platform::set_dpi_awareness();
    // Opt into dark mode before any window or menu exists, so native surfaces
    // (tray context menu, settings title bar) match the app's dark theme.
    #[cfg(target_os = "windows")]
    platform::enable_dark_mode();

    let storage = Rc::new(RefCell::new(
        steward_storage::Storage::open().expect("failed to open the Steward storage database"),
    ));
    let language = storage.borrow().get_setting(LANGUAGE_SETTING);
    let i18n = Rc::new(
        i18n::Localization::new_with_language(language.as_deref())
            .expect("failed to initialize localization"),
    );
    let state = Rc::new(RefCell::new(LauncherState {
        window: None,
        settings_window: None,
        focus: None,
        result_count: 0,
        last_applied_height: 0.0,
        storage,
        engine: Rc::new(RefCell::new(Engine::new())),
        icon_cache: RefCell::new(HashMap::new()),
        scan_rx: RefCell::new(None),
    }));

    // GPUI starts at boot and the launcher window is created hidden, so every
    // summon is instant. A lazy-loading variant was measured and reverted: it
    // deferred GPUI/DirectX/DirectWrite initialization to the first summon,
    // making that summon 0.5-4.5 s — unacceptable for a launcher (decision
    // record in docs/architecture.md).
    application().run(move |cx: &mut App| {
        let focus = init_ui_common(cx, &state);

        // Seed the index from the cache, and refresh it in the background when
        // the cache is stale, before the window opens.
        state.borrow().ensure_app_index();
        let window = open_launcher_window(cx, &focus, i18n.clone(), &state);
        state.borrow_mut().window = Some(window);

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Err(error) = setup_tray(&i18n) {
            eprintln!("failed to create tray icon: {error:#}");
        }
        match register_global_hotkey() {
            Ok(manager) => {
                // The hidden hotkey window must stay alive for the app
                // lifetime.
                Box::leak(Box::new(manager));
            }
            Err(error) => eprintln!("failed to register global hotkey: {error:#}"),
        }
        if let Err(error) = spawn_event_poll_task(state.clone(), i18n.clone(), cx) {
            eprintln!("failed to start event polling task: {error:#}");
        }
    });
}

/// Common GPUI bootstrap shared by every startup path: key bindings,
/// gpui-component init, theme, focus handle and the launcher-window close
/// subscription.
fn init_ui_common(cx: &mut App, state: &Rc<RefCell<LauncherState>>) -> FocusHandle {
    // The tray icon is the application shell: closing the launcher window
    // (Esc, activation loss, Alt+F4) must not terminate the process. GPUI
    // defaults to quitting when the last window closes on non-macOS.
    cx.set_quit_mode(QuitMode::Explicit);

    cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);

    // Initialize the gpui-component stack (theme, global state, root,
    // popover/menu layers, ...) before the first UI element renders; the
    // settings window's `Settings` widget depends on it.
    steward_ui_components::init_components(cx);

    // Match the component surfaces (settings window, ...) to the launcher's
    // palette and restore the persisted theme color.
    let accent = state
        .borrow()
        .storage
        .borrow()
        .get_setting(THEME_COLOR_SETTING)
        .and_then(|value| parse_hex_color(&value))
        .unwrap_or(DEFAULT_ACCENT);
    apply_steward_theme(cx, accent);

    let focus = cx.focus_handle();
    state.borrow_mut().focus = Some(focus.clone());

    // A background scan may have finished while the app was starting; apply it
    // before the first window opens.
    state.borrow().apply_scan_results();

    // Closing the launcher (e.g. Alt+F4) must not kill the app: the tray icon
    // is the application shell, and the window is reopened on demand. Filtered
    // by window id so settings-window close events don't clear the launcher
    // handle.
    let closed_state = state.clone();
    cx.on_window_closed(move |_cx, window_id| {
        let launcher_id = closed_state.borrow().window.as_ref().map(|h| h.window_id());
        if launcher_id == Some(window_id) {
            closed_state.borrow_mut().window = None;
        }
    })
    .detach();

    focus
}

fn open_launcher_window(
    cx: &mut App,
    focus: &FocusHandle,
    i18n: Rc<i18n::Localization>,
    state: &Rc<RefCell<LauncherState>>,
) -> AnyWindowHandle {
    // Frosted-glass window background. Windows 11 renders its native Mica
    // material via DWM (the legacy acrylic blur-behind API is unreliable on
    // 22H2+); macOS and Linux use the native vibrancy / KDE blur instead.
    #[cfg(target_os = "windows")]
    let window_background = WindowBackgroundAppearance::MicaBackdrop;
    #[cfg(not(target_os = "windows"))]
    let window_background = WindowBackgroundAppearance::Blurred;

    let bounds = Bounds::centered(None, size(px(LAUNCHER_WIDTH), px(LAUNCHER_HEIGHT)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Hide the system titlebar so the bar is borderless.
            titlebar: Some(TitlebarOptions {
                appears_transparent: true,
                ..Default::default()
            }),
            // Silent startup: the window exists but stays hidden until the
            // hotkey or tray summons it.
            show: false,
            focus: false,
            // PopUp = tool window on Windows: no taskbar entry, always on top.
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            window_background,
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("Steward");

            // The shared index was seeded in `ensure_app_index` at boot and is
            // refreshed in the background; the window only takes a handle to
            // it. Mirror the current entries into the delegate so Enter can
            // resolve a row to its path.
            let storage = state.borrow().storage.clone();
            let engine = state.borrow().engine.clone();
            let last_results = Rc::new(RefCell::new(engine.borrow().entries().to_vec()));
            let results_for_cb = last_results.clone();
            let confirm_storage = storage.clone();
            let on_confirm = move |index: usize| {
                let entry = results_for_cb.borrow().get(index).cloned();
                if let Some(app) = entry {
                    let _ = confirm_storage.borrow().upsert_usage(&app.path);
                    if let Err(error) = launch(&app.path) {
                        eprintln!("failed to launch {}: {error:#}", app.path.display());
                    }
                }
            };
            let delegate = ResultListDelegate::new()
                .type_label(i18n.translate("application"))
                .on_confirm(on_confirm);

            cx.new(|cx| {
                focus.focus(window, cx);
                // Dismiss the launcher whenever another application takes
                // activation (e.g. the user clicks another window).
                let activation_subscription =
                    cx.observe_window_activation(window, |_, window, cx| {
                        if !window.is_window_active() {
                            hide_window(window, cx);
                        }
                    });
                let results = ResultList::new(delegate, window, cx);
                let mut app = StewardApp {
                    focus_handle: focus.clone(),
                    input: SearchInput {
                        query: String::new(),
                        cursor: 0,
                        marked: None,
                    },
                    i18n,
                    engine,
                    storage: storage.clone(),
                    last_results,
                    results,
                    state: state.clone(),
                    _activation_subscription: activation_subscription,
                };
                // Seed the first summon with the most-used applications (an
                // empty query sorts by usage frequency), so the launcher opens
                // with recents instead of an empty bar.
                app.search(window, cx);
                app
            })
        },
    )
    .expect("failed to open the launcher window")
    .into()
}

/// Settings window built on `gpui_component`'s `Settings` widget.
///
/// Pages:
/// - General: launch-at-startup switch (the single home for autostart; the
///   tray menu no longer carries a duplicate toggle).
/// - About: app name, version, and a short description.
struct SettingsApp {
    i18n: Rc<i18n::Localization>,
    storage: Rc<RefCell<steward_storage::Storage>>,
    state: Rc<RefCell<LauncherState>>,
    /// Current theme accent color as `0xRRGGBB`; drives the active swatch.
    accent: u32,
    /// Currently selected language code (e.g. `zh`), drives the dropdown.
    language: String,
}

impl Render for SettingsApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let view_autostart = view.clone();
        let view_theme_value = view.clone();
        let view_theme_set = view.clone();
        let view_language_value = view.clone();
        let view_language_set = view.clone();
        let i18n = self.i18n.clone();
        let state = self.state.clone();
        let storage = self.storage.clone();
        let storage_language = storage.clone();
        let general_title = self.i18n.translate("settings-general");
        let autostart_label = self.i18n.translate("app-autostart");
        let theme_title = self.i18n.translate("settings-theme");
        let language_title = self.i18n.translate("settings-language");
        let about_title = self.i18n.translate("settings-about");
        let version_label = self.i18n.translate("settings-version");
        let language_options: Vec<(SharedString, SharedString)> = SUPPORTED_LANGUAGES
            .iter()
            .map(|(code, name)| (SharedString::from(*code), SharedString::from(*name)))
            .collect();
        let theme_options: Vec<(SharedString, SharedString)> = ACCENT_PRESETS
            .iter()
            .map(|&color| {
                (
                    SharedString::from(format!("#{color:06x}")),
                    SharedString::from(self.i18n.translate(accent_label_key(color))),
                )
            })
            .collect();

        // The keyed state id includes the language so switching locale
        // rebuilds the Settings widget's search input with a translated
        // placeholder.
        Settings::new(format!("steward-settings-{}", self.language))
            .sidebar_width(px(200.0))
            .sidebar_size_range(px(160.0)..px(280.0))
            .pages(vec![
                SettingPage::new(general_title)
                    .default_open(true)
                    .icon(Icon::new(IconName::Settings2))
                    .groups(vec![SettingGroup::new()
                        .item(SettingItem::new(
                            language_title,
                            SettingField::render(move |_options, _window, cx| {
                                let code = view_language_value.read(cx).language.clone();
                                let current_label = SUPPORTED_LANGUAGES
                                    .iter()
                                    .find(|(c, _)| *c == code)
                                    .map(|(_, name)| SharedString::from(*name))
                                    .unwrap_or_else(|| SharedString::from(code.clone()));
                                let on_select = {
                                    let storage_language = storage_language.clone();
                                    let i18n = i18n.clone();
                                    let state = state.clone();
                                    let view = view_language_set.clone();
                                    move |code: SharedString, cx: &mut App| {
                                        let _ = storage_language
                                            .borrow()
                                            .set_setting(LANGUAGE_SETTING, &code);
                                        i18n.select_language(&code);
                                        // Keep gpui-component's own widgets (the
                                        // settings search box) in sync, and
                                        // refresh the launcher's row type label.
                                        gpui_component::set_locale(gpui_component_locale(&code));
                                        update_launcher_label(&state, cx);
                                        view.update(cx, |app, cx| {
                                            app.language = code.to_string();
                                            cx.notify();
                                        });
                                    }
                                };
                                dropdown_field(
                                    "language-dropdown",
                                    SharedString::from(code),
                                    current_label,
                                    language_options.clone(),
                                    on_select,
                                )
                            }),
                        ))
                        .item(SettingItem::new(
                            theme_title,
                            SettingField::render(move |_options, _window, cx| {
                                let accent = view_theme_value.read(cx).accent;
                                let value = SharedString::from(format!("#{accent:06x}"));
                                let current_label = theme_options
                                    .iter()
                                    .find(|(option_value, _)| *option_value == value)
                                    .map(|(_, label)| label.clone())
                                    .unwrap_or_else(|| value.clone());
                                let on_select = {
                                    let storage = storage.clone();
                                    let view = view_theme_set.clone();
                                    move |hex: SharedString, cx: &mut App| {
                                        if let Some(color) = parse_hex_color(&hex) {
                                            let _ = storage
                                                .borrow()
                                                .set_setting(THEME_COLOR_SETTING, &hex);
                                            apply_steward_theme(cx, color);
                                            view.update(cx, |app, cx| {
                                                app.accent = color;
                                                cx.notify();
                                            });
                                        }
                                    }
                                };
                                dropdown_field(
                                    "theme-dropdown",
                                    value,
                                    current_label,
                                    theme_options.clone(),
                                    on_select,
                                )
                            }),
                        ))
                        .item(SettingItem::new(
                            autostart_label,
                            SettingField::switch(
                                |_cx: &App| autostart_enabled(),
                                move |enabled: bool, cx: &mut App| {
                                    // Write the registry, then re-render so the
                                    // switch reflects the state that actually
                                    // took effect.
                                    set_autostart(enabled);
                                    view_autostart.update(cx, |_, cx| cx.notify());
                                },
                            )
                            .default_value(false),
                        ))]),
                SettingPage::new(about_title)
                    .resettable(false)
                    .icon(Icon::new(IconName::Info))
                    .group(SettingGroup::new().item(SettingItem::render(
                        move |_options, _window, cx| {
                            v_flex()
                                .gap_3()
                                .w_full()
                                .items_center()
                                .child(
                                    Icon::new(IconName::GalleryVerticalEnd)
                                        .size_16()
                                        .text_color(cx.theme().primary),
                                )
                                .child(div().text_lg().font_semibold().child("Steward"))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{version_label} {}",
                                            env!("CARGO_PKG_VERSION")
                                        )),
                                )
                        },
                    ))),
            ])
    }
}

/// i18n key for an accent preset's localized name.
fn accent_label_key(color: u32) -> &'static str {
    match color {
        0x94e2d5 => "settings-theme-jade",
        0xf38ba8 => "settings-theme-rose",
        0xf9e2af => "settings-theme-amber",
        _ => "settings-theme-blue",
    }
}

/// Map a Steward language code to the gpui-component locale. The component
/// bundles only `en`/`zh-CN` (plus a few others); anything unsupported falls
/// back to English inside the component.
fn gpui_component_locale(code: &str) -> &'static str {
    match code {
        "zh" => "zh-CN",
        _ => "en",
    }
}

/// Refresh the launcher's localized row type label ("应用"/"Application")
/// after the UI language changes. The label is stored state in the results
/// list, so it needs an explicit update even though the shared i18n loader
/// already switched.
fn update_launcher_label(state: &Rc<RefCell<LauncherState>>, cx: &mut App) {
    let Some(window) = state.borrow().window else {
        return;
    };
    let Some(app) = window.downcast::<StewardApp>() else {
        return;
    };
    let _ = app.update(cx, |app, _window, cx| {
        let label = app.i18n.translate("application");
        app.results.set_type_label(label, cx);
        cx.notify();
    });
}

/// A settings dropdown control: a fixed-width outline button whose label is
/// centered (with a trailing caret), opening a popup menu of `options`.
/// Unlike gpui-component's built-in field dropdown, every instance has the
/// same width and the text is centered instead of left-aligned next to the
/// caret.
fn dropdown_field(
    id: &'static str,
    current_value: SharedString,
    current_label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    on_select: impl Fn(SharedString, &mut App) + 'static,
) -> impl IntoElement {
    let on_select = Rc::new(on_select);
    Button::new(id)
        .label(SharedString::from(format!("{current_label}  ▾")))
        .outline()
        .w(px(150.0))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let on_select = on_select.clone();
            options.iter().fold(menu, |menu, (value, label)| {
                let on_select = on_select.clone();
                let value = value.clone();
                menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(value == current_value)
                        .on_click(move |_, _, cx| on_select(value.clone(), cx)),
                )
            })
        })
        .into_any_element()
}

/// Open (or focus) the settings window. Keeps `LauncherState.settings_window`
/// in sync: the handle is cleared when the window is closed so a later menu
/// click reopens it instead of touching a stale handle.
fn open_settings_window(
    cx: &mut App,
    i18n: Rc<i18n::Localization>,
    state: &Rc<RefCell<LauncherState>>,
) -> AnyWindowHandle {
    // Wide enough that the settings page content stays above gpui-component's
    // 480px stacked-layout threshold with the default sidebar, keeping every
    // setting item on a single row (title left, control right).
    let bounds = Bounds::centered(None, size(px(800.0), px(480.0)), cx);
    let handle: AnyWindowHandle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Keep the settings window from shrinking below its default
                // size; the page content must stay above gpui-component's
                // 480px stacked-layout threshold for single-row items.
                window_min_size: Some(size(px(800.0), px(480.0))),
                titlebar: Some(TitlebarOptions {
                    appears_transparent: false,
                    ..Default::default()
                }),
                show: true,
                focus: true,
                kind: WindowKind::Normal,
                is_resizable: true,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title(&i18n.translate("app-settings"));
                // The settings panel uses gpui-component widgets (input,
                // tooltip, menu popovers), so the window's root must be a
                // `Root`: it owns the overlay layers those widgets render into.
                let storage = state.borrow().storage.clone();
                let accent = storage
                    .borrow()
                    .get_setting(THEME_COLOR_SETTING)
                    .and_then(|value| parse_hex_color(&value))
                    .unwrap_or(DEFAULT_ACCENT);
                let language = storage
                    .borrow()
                    .get_setting(LANGUAGE_SETTING)
                    .unwrap_or_else(|| i18n.language());
                let view = cx.new(move |_| SettingsApp {
                    i18n,
                    storage,
                    state: state.clone(),
                    accent,
                    language,
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open the settings window")
        .into();

    let settings_id = handle.window_id();
    let closed_state = state.clone();
    cx.on_window_closed(move |_cx, window_id| {
        if window_id == settings_id {
            closed_state.borrow_mut().settings_window = None;
        }
    })
    .detach();

    handle
}

/// Whether Steward is registered to launch at logon. Windows reads the
/// per-user `Run` key; other platforms are stubbed until M4.
#[cfg(target_os = "windows")]
fn autostart_enabled() -> bool {
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
fn autostart_enabled() -> bool {
    false
}

/// Register (`enabled`) or unregister Steward at logon and return the state
/// that actually took effect, so the tray check mark always mirrors reality.
#[cfg(target_os = "windows")]
fn set_autostart(enabled: bool) -> bool {
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
fn set_autostart(_enabled: bool) -> bool {
    false
}

/// Create and register the global hotkey. The manager must be kept alive for
/// the process lifetime (callers `Box::leak` it); its hidden window delivers
/// `WM_HOTKEY` to whichever message pump owns the thread.
fn register_global_hotkey() -> Result<GlobalHotKeyManager> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
    manager.register(hotkey).context("register global hotkey")?;
    Ok(manager)
}

/// Bridge native tray/hotkey events into the GPUI event loop. Runs only after
/// GPUI started; the hotkey manager itself is registered by the caller
/// (boot closure).
fn spawn_event_poll_task(
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut App,
) -> Result<()> {
    let hotkey_events = GlobalHotKeyEvent::receiver();

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let tray_events = TrayIconEvent::receiver();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let menu_events = MenuEvent::receiver();

    cx.spawn(async move |cx| loop {
        // A background scan may finish at any time; both event loops drain it.
        state.borrow().apply_scan_results();

        while let Ok(event) = hotkey_events.try_recv() {
            if event.state == HotKeyState::Pressed {
                toggle_launcher(&state, i18n.clone(), cx);
            }
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        while let Ok(event) = tray_events.try_recv() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                toggle_launcher(&state, i18n.clone(), cx);
            }
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        while let Ok(event) = menu_events.try_recv() {
            match event.id().as_ref() {
                MENU_SETTINGS => {
                    let state_ref = state.borrow();
                    if let Some(handle) = state_ref.settings_window.as_ref() {
                        let _ = handle.update(cx, |_, window, cx| {
                            cx.activate(true);
                            window.refresh();
                        });
                    } else {
                        drop(state_ref);
                        let handle = cx.update(|cx| open_settings_window(cx, i18n.clone(), &state));
                        state.borrow_mut().settings_window = Some(handle);
                    }
                }
                MENU_QUIT => cx.update(|cx| cx.quit()),
                _ => {}
            }
        }

        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    })
    .detach();

    Ok(())
}

/// Summon or dismiss the launcher bar. Reopens the window if it was closed.
fn toggle_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut AsyncApp,
) {
    let mut state_ref = state.borrow_mut();
    match state_ref.window {
        Some(handle) => {
            let focus = state_ref
                .focus
                .clone()
                .expect("focus is initialized together with GPUI");
            let height = state_ref.height();
            state_ref.last_applied_height = height;
            let _ = handle.update(cx, |_, window, cx| {
                if platform::is_visible(window) {
                    hide_window(window, cx);
                } else {
                    // Re-apply the height so a freshly-created window matches
                    // the current result count (mirrors live sizing on search).
                    platform::resize(window, height);
                    focus.focus(window, cx);
                    show_window(window, cx);
                }
            });
        }
        None => {
            drop(state_ref);
            show_launcher(state, i18n, cx);
        }
    }
}

/// Show the launcher bar, reopening the window first if necessary.
fn show_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut AsyncApp,
) {
    if state.borrow().window.is_none() {
        let focus = state
            .borrow()
            .focus
            .clone()
            .expect("focus is initialized together with GPUI");
        let handle = cx.update(|cx| open_launcher_window(cx, &focus, i18n.clone(), state));
        state.borrow_mut().window = Some(handle);
    }
    let mut state_ref = state.borrow_mut();
    if let Some(handle) = state_ref.window {
        let focus = state_ref
            .focus
            .clone()
            .expect("focus is initialized together with GPUI");
        let height = state_ref.height();
        state_ref.last_applied_height = height;
        let _ = handle.update(cx, |_, window, cx| {
            platform::resize(window, height);
            focus.focus(window, cx);
            show_window(window, cx);
        });
    }
}

fn hide_window(window: &mut Window, _cx: &mut App) {
    #[cfg(target_os = "windows")]
    if CLOSE_ON_HIDE {
        // Remove the window entirely instead of hiding it: the per-window
        // renderer, swapchain and GPU atlas are reclaimed, and the next summon
        // recreates the window from the shared engine (no re-scan). The shared
        // handle is cleared by the `on_window_closed` subscription.
        window.remove_window();
        return;
    }
    platform::hide(window);
    #[cfg(not(target_os = "windows"))]
    _cx.hide();
}

fn show_window(window: &mut Window, _cx: &mut App) {
    #[cfg(not(target_os = "windows"))]
    _cx.activate(true);
    platform::show(window);
    window.refresh();
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn setup_tray(i18n: &i18n::Localization) -> Result<()> {
    let icon = load_tray_icon()?;

    // The tray menu is deliberately minimal: the autostart toggle lives in the
    // settings window, so the menu only carries Settings and Quit.
    let settings = MenuItem::with_id(MENU_SETTINGS, i18n.translate("app-settings"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, i18n.translate("app-quit"), true, None);
    let separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    menu.append(&settings)?;
    menu.append(&separator)?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("Steward")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .context("build system tray icon")?;
    // The tray icon must outlive the event loop.
    Box::leak(Box::new(tray));

    Ok(())
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> Result<TrayIcon> {
    // Resource 2 is the dark tray variant (generated by
    // `scripts/generate-icons.py`); resource 1 stays the app/taskbar icon.
    TrayIcon::from_resource(2, Some((32, 32)))
        .context("load dark tray icon from embedded resources")
}

#[cfg(target_os = "macos")]
fn load_tray_icon() -> Result<TrayIcon> {
    let png = include_bytes!("../../assets/steward-dark.png");
    let image = image::load_from_memory(png).context("decode bundled steward-dark.png")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    TrayIcon::from_rgba(rgba.into_raw(), width, height).context("create macOS tray icon")
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::time::Duration;
    use windows_sys::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::HiDpi::GetDpiForWindow,
        UI::WindowsAndMessaging::{
            GetCaretBlinkTime, GetClientRect, GetForegroundWindow, GetWindowRect,
            GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SetWindowPos,
            ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
        },
    };

    /// Declare PerMonitorV2 DPI awareness (Windows 10 1703+). Without this the
    /// OS virtualizes the process at 96 DPI and upscales the window, which
    /// breaks both the launcher's physical size and its dynamic resize. The
    /// call is a no-op (returns false) when the executable already declares
    /// awareness via its manifest, which is fine: the process is aware either
    /// way.
    pub fn set_dpi_awareness() {
        unsafe {
            use windows_sys::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
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

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn is_visible(window: &Window) -> bool {
        hwnd(window).is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 })
    }

    pub fn hide(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    pub fn show(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            position_centered(hwnd);
            ShowWindow(hwnd, SW_SHOW);
            force_foreground(hwnd);
        }
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

    /// Convert a desired logical client size into the physical window size
    /// including the native non-client frame. The launcher is borderless, but
    /// Windows still frames WS_POPUP windows with a few device pixels; if they
    /// are not added, the client area ends up shorter than requested and the
    /// flex column clips the drop-down's last row.
    unsafe fn client_to_window_px(hwnd: HWND, width: f32, height: f32) -> (i32, i32) {
        let dpi = GetDpiForWindow(hwnd).max(96);
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
            let (width_px, height_px) = client_to_window_px(hwnd, crate::LAUNCHER_WIDTH, height);
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

    /// A window opened with `show: false` never receives the placement GPUI
    /// computed for it, so the launcher keeps its creation-time (default)
    /// bounds — which can end up as an OS default size when the requested
    /// bounds are rejected. Apply the launcher's own physical size and
    /// position on every show: fixed design width, current result height
    /// (the caller resizes it before showing), centered horizontally and in
    /// the upper third of the work area.
    unsafe fn position_centered(hwnd: HWND) {
        use crate::LAUNCHER_WIDTH;
        let dpi = GetDpiForWindow(hwnd).max(96);
        let scale = dpi as f32 / 96.0;
        let mut client_rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client_rect);
        // The client height is already the current drop-down height in
        // physical px; recover it in logical px and re-derive the full window
        // size including borders so the client area stays exactly on-design.
        let height_logical = (client_rect.bottom as f32 / scale).max(1.0);
        let (width, height) = client_to_window_px(hwnd, LAUNCHER_WIDTH, height_logical);

        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - width) / 2;
        // Horizontally centered; vertically in the upper third of the screen.
        let y = work.top + ((work.bottom - work.top) - height) / 3;

        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
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
mod platform {
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

    pub fn show(_window: &Window) {
        WINDOW_VISIBLE.store(true, Ordering::Relaxed);
    }

    /// Resizing is a Windows-specific launcher behavior for now.
    pub fn resize(_window: &Window, _height: f32) {}

    pub fn caret_blink_period() -> Duration {
        Duration::from_millis(1060)
    }
}

#[cfg(test)]
mod tests {
    use super::SearchInput;

    fn input(query: &str) -> SearchInput {
        SearchInput {
            query: query.to_string(),
            cursor: query.chars().count(),
            marked: None,
        }
    }

    #[test]
    fn ime_commit_appends_at_caret() {
        let mut input = input("你好");
        input.replace_utf16(None, "说话");
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.cursor, 4);
        assert!(input.marked.is_none());
    }

    #[test]
    fn ime_composition_replaces_only_the_marked_text() {
        let mut input = input("你好");
        // The IME starts composing "说话" after the existing text.
        input.replace_and_mark_utf16(None, "说话", Some(0..2));
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.marked, Some(2..4));

        // The commit replaces just the marked range, keeping the prefix.
        input.replace_utf16(None, "说话");
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.cursor, 4);
        assert!(input.marked.is_none());
    }

    #[test]
    fn ime_commit_inserts_at_caret_in_the_middle() {
        let mut input = input("你好世界");
        input.cursor = 2;
        input.replace_utf16(None, "呀");
        assert_eq!(input.query, "你好呀世界");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn ime_cancel_clears_only_the_composition() {
        let mut input = input("你好");
        input.replace_and_mark_utf16(None, "ni", None);
        assert_eq!(input.query, "你好ni");
        assert_eq!(input.marked, Some(2..4));

        // lparam == 0 cancels the composition with an empty replacement.
        input.replace_utf16(None, "");
        assert_eq!(input.query, "你好");
        assert!(input.marked.is_none());
    }

    #[test]
    fn ascii_replacement_at_caret() {
        let mut input = input("abc");
        input.replace_utf16(None, "d");
        assert_eq!(input.query, "abcd");
        assert_eq!(input.cursor, 4);
    }
}
