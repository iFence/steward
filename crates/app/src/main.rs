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
    actions, div, point, prelude::*, px, rgb, size, Animation, AnimationExt, AnyElement,
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Div, Element, ElementId,
    ElementInputHandler, EntityInputHandler, FocusHandle, GlobalElementId, InspectorElementId,
    KeyBinding, KeyDownEvent, LayoutId, Pixels, Subscription, TitlebarOptions, UTF16Selection,
    Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions,
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
    menu::{Menu, MenuEvent, MenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
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

const MENU_TOGGLE: &str = "toggle";
const MENU_QUIT: &str = "quit";

/// Height of the results drop-down for `count` visible rows, capped at
/// `MAX_RESULT_ROWS` so the window stops growing once the list scrolls.
fn result_height(count: usize) -> f32 {
    RESULT_ROW_HEIGHT * count.min(MAX_RESULT_ROWS) as f32
}

/// Total launcher window height for a given number of visible result rows:
/// the input bar plus the (possibly scroll-capped) drop-down.
fn launcher_height(result_count: usize) -> f32 {
    LAUNCHER_HEIGHT + result_height(result_count)
}

/// Spawn the application at `path`, detaching from the launcher process so
/// both keep running independently.
fn launch(path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
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
    /// Extracted app icons (PNG-encoded `gpui::Image`), keyed by app path.
    /// `None` entries mark paths whose icon could not be extracted.
    icon_cache: RefCell<HashMap<std::path::PathBuf, Option<Arc<gpui::Image>>>>,
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
    focus: FocusHandle,
    result_count: usize,
}

impl LauncherState {
    /// Total launcher window height for the current result count: the input
    /// bar plus the result drop-down.
    fn height(&self) -> f32 {
        launcher_height(self.result_count)
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
        let root = div()
            .track_focus(&self.focus_handle)
            .on_action(|_: &HideWindow, window, cx| hide_window(window, cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x232332))
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
                                        .child(cursor())
                                        .child(
                                            div()
                                                .text_color(rgb(0x6c7086))
                                                .child(self.i18n.translate("search-placeholder")),
                                        )
                                } else {
                                    self.render_query_text()
                                },
                            ),
                    )
                    .child(drag_strip().w(px(LAUNCHER_MARGIN))),
            )
            .when(result_count > 0, |this| {
                // Pin the drop-down to exactly its result height so it never
                // grows into the input bar, and inset it by the same margin as
                // the drag strips so rows align with the bar content.
                let drop_height = result_height(result_count);
                this.child(
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
    fn render_query_text(&self) -> Div {
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
        children.push(cursor().into_any_element());
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
        // below the fold render without an icon for now.
        let icons = items
            .iter()
            .take(MAX_RESULT_ROWS)
            .map(|app| {
                let cached = self.icon_cache.borrow().get(&app.path).cloned();
                cached.unwrap_or_else(|| {
                    let icon = app_icons::app_icon_image(&app.path);
                    self.icon_cache
                        .borrow_mut()
                        .insert(app.path.clone(), icon.clone());
                    icon
                })
            })
            .collect::<Vec<_>>();
        let missing = items.len().saturating_sub(icons.len());
        let icons = icons
            .into_iter()
            .chain(std::iter::repeat_n(None, missing))
            .collect::<Vec<_>>();

        *self.last_results.borrow_mut() = items.clone();
        self.results.set_results(items, icons, cx);

        let count = self.results.visible_count(cx);
        self.state.borrow_mut().result_count = count;
        // Resize through GPUI's own API: its Windows path schedules the native
        // resize on the foreground executor, which keeps the DirectX renderer
        // viewport in sync (a raw SetWindowPos while visible leaves the
        // drop-down unpainted on this GPUI revision).
        window.resize(size(px(LAUNCHER_WIDTH), px(launcher_height(count))));
        cx.notify();
    }

    /// Reset the launcher to its idle (bar-only) state and hide it. Called
    /// after the delegate's confirm callback has launched the selected app.
    fn after_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.query.clear();
        self.input.cursor = 0;
        self.input.marked = None;
        self.results.set_results(Vec::new(), Vec::new(), cx);
        self.state.borrow_mut().result_count = 0;
        window.resize(size(px(LAUNCHER_WIDTH), px(LAUNCHER_HEIGHT)));
        hide_window(window, cx);
        cx.notify();
    }
}

/// A blinking text cursor rendered as a thin vertical bar, using the same
/// on/off cadence as the OS caret.
fn cursor() -> impl IntoElement {
    div()
        .w(px(2.0))
        .h(px(18.0))
        .bg(rgb(0x89b4fa))
        .with_animation(
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

    application().run(|cx: &mut App| {
        cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);

        // Register the gpui-component theme global before the first UI element
        // (a `ListItem`) renders; without it the first paint panics.
        steward_ui_components::init_theme(cx);

        let i18n = Rc::new(i18n::Localization::new().expect("failed to initialize localization"));
        let focus = cx.focus_handle();
        let state = Rc::new(RefCell::new(LauncherState {
            window: None,
            focus: focus.clone(),
            result_count: 0,
        }));
        let window = open_launcher_window(cx, &focus, i18n.clone(), &state);
        state.borrow_mut().window = Some(window);

        // Closing the launcher (e.g. Alt+F4) must not kill the app: the tray
        // icon is the application shell, and the window is reopened on demand.
        let closed_state = state.clone();
        cx.on_window_closed(move |_cx, _window_id| {
            closed_state.borrow_mut().window = None;
        })
        .detach();

        if let Err(error) = setup_global_hotkey(state.clone(), i18n.clone(), cx) {
            eprintln!("failed to register global hotkey: {error:#}");
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Err(error) = setup_tray(cx, &i18n) {
            eprintln!("failed to create tray icon: {error:#}");
        }
    });
}

fn open_launcher_window(
    cx: &mut App,
    focus: &FocusHandle,
    i18n: Rc<i18n::Localization>,
    state: &Rc<RefCell<LauncherState>>,
) -> AnyWindowHandle {
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
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("Steward");

            // Build the shared app index from a fresh scan (falling back to the
            // SQLite cache if the scan yields nothing), then mirror it into the
            // delegate so Enter can resolve a row to its path.
            let storage = Rc::new(RefCell::new(
                steward_storage::Storage::open()
                    .expect("failed to open the Steward storage database"),
            ));
            let apps = load_apps(&storage);
            let mut engine = Engine::new();
            engine.set_entries(apps.clone());
            if let Err(error) = storage.borrow_mut().mark_seen(&apps) {
                eprintln!("failed to refresh app cache: {error:#}");
            }

            // Mirror the index into the confirm callback so it can resolve a
            // row index to an app path at Enter time. The callback deliberately
            // touches only Rc-shared state (never the UI), so it is safe to run
            // from inside the ListState update; the app-level `after_confirm`
            // resets and hides afterward.
            let last_results = Rc::new(RefCell::new(apps));
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
                    engine: Rc::new(RefCell::new(engine)),
                    storage: storage.clone(),
                    last_results,
                    icon_cache: RefCell::new(HashMap::new()),
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

/// Resolve the set of applications to index: scan the platform if non-empty,
/// otherwise fall back to the last cached scan so cold start never blocks on a
/// broken scan.
fn load_apps(
    storage: &Rc<RefCell<steward_storage::Storage>>,
) -> Vec<steward_core_engine::AppEntry> {
    let scanned = steward_core_engine::platform_scanner().scan();
    if !scanned.is_empty() {
        return scanned;
    }
    storage.borrow().cached_apps().unwrap_or_default()
}

fn setup_global_hotkey(
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut App,
) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
    manager.register(hotkey).context("register global hotkey")?;
    // The hidden message window must stay alive for the app lifetime.
    Box::leak(Box::new(manager));

    let hotkey_events = GlobalHotKeyEvent::receiver();

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let tray_events = TrayIconEvent::receiver();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let menu_events = MenuEvent::receiver();

    cx.spawn(async move |cx| loop {
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
                MENU_TOGGLE => toggle_launcher(&state, i18n.clone(), cx),
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
    let state_ref = state.borrow();
    match state_ref.window.as_ref() {
        Some(handle) => {
            let focus = state_ref.focus.clone();
            let height = state_ref.height();
            let _ = (*handle).update(cx, |_, window, cx| {
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
        let focus = state.borrow().focus.clone();
        let handle = cx.update(|cx| open_launcher_window(cx, &focus, i18n.clone(), state));
        state.borrow_mut().window = Some(handle);
    }
    let state_ref = state.borrow();
    if let Some(handle) = state_ref.window.as_ref() {
        let focus = state_ref.focus.clone();
        let height = state_ref.height();
        let _ = (*handle).update(cx, |_, window, cx| {
            platform::resize(window, height);
            focus.focus(window, cx);
            show_window(window, cx);
        });
    }
}

fn hide_window(window: &mut Window, _cx: &mut App) {
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
fn setup_tray(_cx: &mut App, i18n: &i18n::Localization) -> Result<()> {
    let icon = load_tray_icon()?;
    let toggle = MenuItem::with_id(MENU_TOGGLE, i18n.translate("app-toggle"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, i18n.translate("app-quit"), true, None);
    let menu = Menu::new();
    menu.append(&toggle)?;
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
fn load_tray_icon() -> Result<Icon> {
    Icon::from_resource(1, Some((32, 32))).context("load tray icon from embedded resources")
}

#[cfg(target_os = "macos")]
fn load_tray_icon() -> Result<Icon> {
    let png = include_bytes!("../../assets/steward.png");
    let image = image::load_from_memory(png).context("decode bundled steward.png")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).context("create macOS tray icon")
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
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::HiDpi::GetDpiForWindow,
        UI::WindowsAndMessaging::{
            GetCaretBlinkTime, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
            IsWindowVisible, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST,
            SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
        },
    };

    /// Declare PerMonitorV2 DPI awareness (Windows 10 1703+). Without this the
    /// OS virtualizes the process at 96 DPI and upscales the window, which
    /// breaks both the launcher's physical size and its dynamic resize.
    pub fn set_dpi_awareness() {
        unsafe {
            use windows_sys::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
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

    /// Resize the launcher window to `height` (CSS/logical px), keeping the
    /// current left/top corner so the drop-down grows downward. Height here is
    /// in DPIs that match the DPI-aware unit GPUI uses; convert to physical px.
    pub fn resize(window: &Window, height: f32) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            let dpi = GetDpiForWindow(hwnd).max(96);
            let scale = dpi as f32 / 96.0;
            let height_px = (height * scale).round() as i32;

            let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
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
        let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);
        let width = (LAUNCHER_WIDTH * scale).round() as i32;
        let height = (rect.bottom - rect.top).max(1);

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
