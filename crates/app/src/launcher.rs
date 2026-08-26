//! The launcher window's model: query buffer handling, search, rendering and
//! the shared state that outlives any single window.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    rc::Rc,
    sync::Arc,
};

use global_hotkey::hotkey::HotKey;
use global_hotkey::GlobalHotKeyManager;
use gpui::{
    div, point, prelude::*, px, rgb, size, AnyElement, Animation, AnimationExt, App, Bounds, Div,
    ClipboardItem, DispatchPhase, Element, ElementId, ElementInputHandler, EntityInputHandler,
    FocusHandle, GlobalElementId, Hsla, InspectorElementId, InteractiveElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Subscription,
    UTF16Selection, Window,
};
use gpui_component::ActiveTheme;
use steward_ui_components::{ResultItem, ResultList};

use crate::config::{
    HideWindow, LAUNCHER_HEIGHT, LAUNCHER_MARGIN, LAUNCHER_WIDTH, MAX_RESULT_ROWS,
    RESULT_ROW_HEIGHT,
};
use crate::hotkeys::keystroke_to_hotkey;
use crate::search_input::SearchInput;
use crate::settings::open_settings_window_from_launcher;
use crate::theme::adaptive_selection_wash;
use crate::window::hide_window;

/// Approximate horizontal pitch of the query glyphs, in logical pixels. Used
/// to map a mouse x coordinate to a character index (and to place the IME
/// candidate window), since the launcher hand-renders its query text instead
/// of using a GPUI text system with metrics.
const GLYPH_WIDTH: f32 = 9.0;
/// The query text's left edge, in window coordinates: the launcher's left
/// drag-strip margin plus the input row's `px_3` padding.
const INPUT_TEXT_X: f32 = LAUNCHER_MARGIN + 12.0;

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

pub(crate) struct StewardApp {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) input: SearchInput,
    pub(crate) i18n: Rc<crate::i18n::Localization>,
    /// Shared search index, rebuilt at startup from a scan / cache.
    pub(crate) engine: Rc<RefCell<steward_core_engine::Engine>>,
    /// SQLite cache plus usage-frequency tracking.
    pub(crate) storage: Rc<RefCell<steward_storage::Storage>>,
    /// Current result rows, mirrored for the `on_confirm` callback to resolve
    /// the selected index into an app path (or a calculator result to copy).
    pub(crate) last_results: Rc<RefCell<Vec<ResultItem>>>,
    pub(crate) results: ResultList,
    /// Result count is published here so the tray/hotkey path can size the
    /// window when it is summoned.
    pub(crate) state: Rc<RefCell<LauncherState>>,
    /// Keeps the window-activation observer alive for the view's lifetime.
    pub(crate) _activation_subscription: Subscription,
    /// Whether a left-button drag is actively extending a text selection in
    /// the query. Set on mouse-down over the input, cleared on mouse-up.
    pub(crate) mouse_selecting: bool,
    /// The character index where the current selection drag started. Only
    /// meaningful while [`Self::mouse_selecting`] is set.
    pub(crate) mouse_anchor: usize,
}

/// A finished background icon extraction: the search generation it belongs to
/// plus the extracted `(path, icon)` pairs, one per pending path.
type IconBatch = (u64, Vec<(std::path::PathBuf, Option<Arc<gpui::Image>>)>);

/// Shared launcher state used by the foreground event loop: the (possibly
/// closed) window handle plus the focus handle that must be re-focused every
/// time the bar is summoned, and the current result count so the drop-down
/// height can be computed at show time.
pub(crate) struct LauncherState {
    pub(crate) window: Option<gpui::AnyWindowHandle>,
    pub(crate) settings_window: Option<gpui::AnyWindowHandle>,
    /// Created together with GPUI (a `FocusHandle` can only be allocated from
    /// an application context); `None` before the first summon.
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) result_count: usize,
    /// Scrim opacity painted over the blurred backdrop, adapted at show time
    /// to the luminance of what sits behind the bar (see
    /// [`crate::theme::adaptive_scrim_alpha`]) so white ink stays readable
    /// over bright backdrops. Defaults to [`palette::SCRIM_ALPHA`].
    pub(crate) scrim_alpha: f32,
    /// Shared SQLite storage (app index cache, usage, persisted settings).
    pub(crate) storage: Rc<RefCell<steward_storage::Storage>>,
    /// Shared search index (entries + pre-built pinyin haystacks), rebuilt
    /// from the SQLite cache at boot and refreshed in the background. Lives
    /// outside the window so recreating the launcher never re-scans.
    pub(crate) engine: Rc<RefCell<steward_core_engine::Engine>>,
    /// Extracted app icons (PNG-encoded `gpui::Image`), keyed by app path.
    /// `None` entries mark paths whose icon could not be extracted. Shared so
    /// a recreated window keeps its icons.
    pub(crate) icon_cache: RefCell<HashMap<std::path::PathBuf, Option<Arc<gpui::Image>>>>,
    /// Generation counter for background icon batches: bumped on every search
    /// so a finished extraction that predates a newer query only fills the
    /// cache instead of re-running the (stale) search.
    pub(crate) icon_gen: Cell<u64>,
    /// Receiver for background icon extractions of results below the fold.
    /// Each search replaces the receiver, so a stale worker's send fails and
    /// its work is dropped with the old channel.
    pub(crate) icon_rx: RefCell<Option<crossbeam_channel::Receiver<IconBatch>>>,
    /// Pending background scan results, drained by the GPUI foreground poll
    /// task.
    pub(crate) scan_rx: RefCell<Option<crossbeam_channel::Receiver<Vec<steward_core_engine::AppEntry>>>>,
    /// Logical launcher height last requested from a resize, so `search` can
    /// skip redundant resize calls when the result-count-driven height has not
    /// changed (e.g. every IME composition update).
    pub(crate) last_applied_height: f32,
    /// Global hotkey manager, created on the event-loop thread so its hidden
    /// window receives `WM_HOTKEY`. Stored here (instead of leaked) so the
    /// settings window can re-register the summon hotkey at runtime.
    pub(crate) hotkey_manager: Option<GlobalHotKeyManager>,
    /// The currently registered summon hotkey, kept so a change can unregister
    /// the old binding and the settings field can display the active one.
    pub(crate) summon_hotkey: Option<HotKey>,
    /// The launcher-scoped settings-window hotkey (persisted, default
    /// `Ctrl+,`). Unlike the summon hotkey it is never registered globally:
    /// the launcher's key handling matches it only while the bar is visible.
    pub(crate) settings_hotkey: Option<HotKey>,
}

impl LauncherState {
    /// Total launcher window height for the current result count: the input
    /// bar plus the result drop-down.
    pub(crate) fn height(&self) -> f32 {
        launcher_height(self.result_count)
    }

    /// Seed the search index from the SQLite cache and, when the cache is
    /// missing or stale, start a background scan whose results are applied by
    /// [`Self::apply_scan_results`]. Never blocks the caller: the scan runs
    /// on a worker thread and both event loops drain the result channel.
    pub(crate) fn ensure_app_index(&self) {
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
    pub(crate) fn apply_scan_results(&self) {
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

/// GPUI's text-input interface. The launcher's query is a single-line
/// document; the Windows platform routes IME composition (Chinese/Japanese/
/// Korean input) and `WM_CHAR` text through these callbacks.
impl EntityInputHandler for StewardApp {
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        Some(self.input.utf16_len())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<UTF16Selection> {
        // Report the active selection (the caret is its head); a plain caret
        // is a zero-length selection.
        let range = match &self.input.selection {
            Some(range) => {
                let start = self.input.char_to_utf16(range.start);
                let end = self.input.char_to_utf16(range.end);
                start..end
            }
            None => {
                let caret = self.input.char_to_utf16(self.input.cursor);
                caret..caret
            }
        };
        let reversed = self
            .input
            .selection
            .as_ref()
            .is_some_and(|range| self.input.cursor == range.start);
        Some(UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
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
        _cx: &mut gpui::Context<Self>,
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
        cx: &mut gpui::Context<Self>,
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
        cx: &mut gpui::Context<Self>,
    ) {
        self.input
            .replace_and_mark_utf16(range, new_text, new_selected_range);
        self.search(window, cx);
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.input.marked = None;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
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
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        // Window-coordinate x, using the same glyph-width estimate as
        // `bounds_for_range` and the mouse-selection handlers.
        let relative = point.x - px(INPUT_TEXT_X);
        let index = (relative / px(GLYPH_WIDTH)).round().max(0.0) as usize;
        Some(index.min(self.input.char_count()))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        // Platforms move the selection on the application's behalf (e.g. a
        // system selection handle); mirror it into the query's own model.
        if range_utf16.start == range_utf16.end {
            if let Some(index) = self.input.utf16_to_char_index(range_utf16.start) {
                self.input.set_cursor(index);
            }
        } else if let Some(range) = self.input.utf16_to_chars(range_utf16) {
            self.input.set_selection(range);
        }
        cx.notify();
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> bool {
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
        self.register_mouse_selection(window);
    }
}

impl IntoElement for LauncherInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Map a window x coordinate to a character index in the query, using the
/// same glyph-width estimate as the IME caret placement and mouse selection.
fn char_index_at_x(bounds: Bounds<Pixels>, query_len: usize, x: Pixels) -> usize {
    let relative = x - bounds.origin.x - px(INPUT_TEXT_X);
    let index = (relative / px(GLYPH_WIDTH)).round().max(0.0) as usize;
    index.min(query_len)
}

impl LauncherInputElement {
    /// Register window-scoped mouse listeners that drive text selection in the
    /// query: press over the input places the caret and anchors the selection,
    /// drag extends it (even when the pointer leaves the input area), release
    /// ends it. The listeners run in the capture phase and never stop
    /// propagation, so clicks on the results drop-down below the bar are
    /// unaffected. Registered every frame because GPUI clears window mouse
    /// listeners after each frame.
    fn register_mouse_selection(&self, window: &mut Window) {
        let input_bounds = self.input_bounds;
        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if event.button != MouseButton::Left || !input_bounds.contains(&event.position) {
                return;
            }
            view.update(cx, |app, cx| {
                let index = char_index_at_x(input_bounds, app.input.char_count(), event.position.x);
                app.begin_mouse_selection(index, cx);
            });
        });

        let input_bounds = self.input_bounds;
        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase != DispatchPhase::Capture || !event.dragging() {
                return;
            }
            view.update(cx, |app, cx| {
                if !app.mouse_selecting {
                    return;
                }
                let index = char_index_at_x(input_bounds, app.input.char_count(), event.position.x);
                app.update_mouse_selection(index, cx);
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            view.update(cx, |app, _| app.end_mouse_selection());
        });
    }
}

impl gpui::Render for StewardApp {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // GPUI's Windows platform disables the IME context from its WM_PAINT
        // path whenever the input handler is momentarily unavailable (taken
        // during the draw). Re-associating every frame keeps composition input
        // available to the launcher.
        #[cfg(target_os = "windows")]
        crate::platform::enable_ime(window);
        #[cfg(not(target_os = "windows"))]
        let _ = window;

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
            // Translucent scrim over the window's blurred backdrop (Windows
            // Acrylic / macOS vibrancy): the launcher keeps its fixed dark
            // look regardless of the system theme, but the frosted glass
            // behind it shows through. The opacity is adapted at show time
            // to the backdrop's luminance (see `adaptive_scrim_alpha`): over
            // a dark desktop it stays at SCRIM_ALPHA, over a bright one it
            // rises toward SCRIM_ALPHA_MAX so the white ink keeps contrast.
            .bg(
                rgb(steward_ui_components::palette::BACKGROUND)
                    .opacity(self.state.borrow().scrim_alpha),
            )
            .text_lg()
            .text_color(rgb(steward_ui_components::palette::FOREGROUND))
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
                                                .text_color(rgb(
                                                    steward_ui_components::palette::MUTED_FOREGROUND,
                                                ))
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
                // A subtle light hairline separates the search box from the
                // results list (Tinycast's `separator`, white 0.10); it only
                // shows while the drop-down is open.
                let drop_height = result_height(result_count);
                this.child(div().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.10)))
                    // Pin the drop-down to exactly its result height so it never
                    // grows into the input bar, and inset it by the same margin as
                    // the drag strips so rows align with the bar content.
                    .child(
                        div()
                            .h(px(drop_height))
                            .mx(px(LAUNCHER_MARGIN))
                            .child(self.results.render(
                                drop_height,
                                adaptive_selection_wash(self.state.borrow().scrim_alpha),
                                cx,
                            )),
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
    /// (underlined), the active text selection (washed), the caret and the
    /// trailing text.
    fn render_query_text(&self, primary: Hsla) -> Div {
        let query = &self.input.query;
        let mut children: Vec<AnyElement> = Vec::new();

        // The active IME composition owns the rendering (underlined text plus
        // caret); a separate selection cannot coexist with a composition.
        if let Some(range) = &self.input.marked {
            let start = self.input.byte_at_char(range.start);
            let end = self.input.byte_at_char(range.end);
            if start > 0 {
                children.push(div().child(query[..start].to_string()).into_any_element());
            }
            children.push(
                div()
                    .underline()
                    .child(query[start..end].to_string())
                    .into_any_element(),
            );
            children.push(cursor(primary).into_any_element());
            if end < query.len() {
                children.push(div().child(query[end..].to_string()).into_any_element());
            }
            return div().flex().flex_row().items_center().children(children);
        }

        if let Some(range) = self.input.selection.clone() {
            // The caret sits at the selection head; the selected span is
            // painted with the same adaptive white wash as the result rows.
            let sel_start = self.input.byte_at_char(range.start);
            let sel_end = self.input.byte_at_char(range.end);
            let caret = self.input.byte_at_char(self.input.cursor);
            let wash = adaptive_selection_wash(self.state.borrow().scrim_alpha);
            let selection_span = |text: &str| {
                div()
                    .bg(rgb(steward_ui_components::palette::SELECTION).opacity(wash))
                    .child(text.to_string())
            };
            if sel_start > 0 {
                children.push(
                    div()
                        .child(query[..sel_start].to_string())
                        .into_any_element(),
                );
            }
            if caret == sel_start {
                // Head at the selection start: caret before the selected text.
                children.push(cursor(primary).into_any_element());
                children.push(selection_span(&query[sel_start..sel_end]).into_any_element());
            } else {
                children.push(selection_span(&query[sel_start..caret]).into_any_element());
                children.push(cursor(primary).into_any_element());
                children.push(selection_span(&query[caret..sel_end]).into_any_element());
            }
            if sel_end < query.len() {
                children.push(div().child(query[sel_end..].to_string()).into_any_element());
            }
        } else {
            // Plain caret between the leading and trailing text.
            let caret = self.input.byte_at_char(self.input.cursor);
            if caret > 0 {
                children.push(div().child(query[..caret].to_string()).into_any_element());
            }
            children.push(cursor(primary).into_any_element());
            if caret < query.len() {
                children.push(div().child(query[caret..].to_string()).into_any_element());
            }
        }

        div().flex().flex_row().items_center().children(children)
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
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

        // Select all (Ctrl+A). The launcher's hand-rolled input owns its
        // selection model, so the standard shortcut has no built-in handler.
        if modifiers.control && !modifiers.alt && !modifiers.platform && keystroke.key == "a" {
            self.input.select_all();
            cx.notify();
            cx.stop_propagation();
            return;
        }

        // Copy / cut the selected query (Ctrl+C / Ctrl+X), like the paste
        // handler below: read/write the platform clipboard directly.
        if modifiers.control && !modifiers.alt && !modifiers.platform && keystroke.key == "c" {
            if let Some(text) = self.input.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            cx.stop_propagation();
            return;
        }

        if modifiers.control && !modifiers.alt && !modifiers.platform && keystroke.key == "x" {
            if let Some(text) = self.input.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.input.delete_selection();
                self.search(window, cx);
            }
            cx.stop_propagation();
            return;
        }

        // Paste from the clipboard. The launcher uses a hand-rolled input, so
        // Ctrl+V has no built-in handler; read the platform clipboard and
        // insert it at the caret (replacing the selection if one is active).
        // Newlines are collapsed to spaces because the query is a single-line
        // document (e.g. copying a path from Explorer).
        if modifiers.control && !modifiers.alt && !modifiers.platform && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
                self.input.insert_str(&text);
                self.search(window, cx);
            }
            cx.stop_propagation();
            return;
        }

        // The settings hotkey (Ctrl+, by default) is launcher-scoped: unlike
        // the summon hotkey it is never registered globally, so it only opens
        // the settings window while the launcher is visible and focused.
        if let Some(hotkey) = keystroke_to_hotkey(keystroke) {
            if self.state.borrow().settings_hotkey == Some(hotkey) {
                open_settings_window_from_launcher(&self.state, self.i18n.clone(), &mut *cx);
                cx.stop_propagation();
                return;
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
    /// resize the window to fit the new drop-down height. A query that is a
    /// complete arithmetic expression additionally gets a calculator row on
    /// top showing the computed value.
    pub(crate) fn search(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let query = self.input.query.clone();

        let mut items: Vec<ResultItem> = Vec::new();
        if let Some(value) = steward_core_engine::calc::try_evaluate(&query) {
            items.push(ResultItem::Action {
                title: steward_core_engine::calc::format_value(value),
                subtitle: query.trim().to_owned(),
            });
        }
        // A URL query (scheme URL, bare domain, IPv4[:port], localhost) is a
        // command: offer an "open in browser" row above any app matches.
        if let Some(url) = steward_core_engine::try_openable(&query) {
            items.push(ResultItem::Link {
                url,
                label: self.i18n.translate("open-in-browser"),
                command_label: self.i18n.translate("command"),
            });
        }

        let apps = self
            .engine
            .borrow()
            .query(&query, &|path| self.storage.borrow().frequency_str(path));

        // Resolve an icon per result, reusing the cache so only new paths pay
        // the (cheap) Win32 extraction cost. The first `MAX_RESULT_ROWS` are
        // extracted synchronously so the visible rows paint immediately; any
        // remaining uncached paths are extracted on a worker thread and
        // applied back to the list once ready (`drain_icon_batches`). The
        // cache lives in the shared launcher state so a recreated window
        // keeps its icons.
        let icon_gen = {
            let state = self.state.borrow_mut();
            let gen = state.icon_gen.get() + 1;
            state.icon_gen.set(gen);
            gen
        };
        let icons = {
            let state = self.state.borrow();
            apps.iter()
                .enumerate()
                .map(|(index, app)| {
                    // `let` ends the temporary borrow before the miss path,
                    // so it can `borrow_mut` again.
                    let cached = state
                        .icon_cache
                        .borrow_mut()
                        .get(&app.path)
                        .cloned()
                        .flatten();
                    if cached.is_some() {
                        return cached;
                    }
                    if index < MAX_RESULT_ROWS {
                        let icon = crate::app_icons::app_icon_image(&app.path);
                        state
                            .icon_cache
                            .borrow_mut()
                            .insert(app.path.clone(), icon.clone());
                        icon
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };

        // Extract the below-the-fold icons off the UI thread so scrolling
        // never stalls on extraction; the foreground poll task applies them
        // (`drain_icon_batches`). Each search replaces the receiver, so a
        // stale worker's send simply fails and its work is dropped.
        {
            let state = self.state.borrow();
            if apps.len() > MAX_RESULT_ROWS {
                let pending = apps[MAX_RESULT_ROWS..]
                    .iter()
                    .filter(|app| !state.icon_cache.borrow().contains_key(&app.path))
                    .map(|app| app.path.clone())
                    .collect::<Vec<_>>();
                if !pending.is_empty() {
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    std::thread::spawn(move || {
                        let icons = pending
                            .into_iter()
                            .map(|path| {
                                let icon = crate::app_icons::app_icon_image(&path);
                                (path, icon)
                            })
                            .collect::<Vec<_>>();
                        let _ = tx.send((icon_gen, icons));
                    });
                    state.icon_rx.borrow_mut().replace(rx);
                }
            }
        }

        // Prepend the calculator row (with no icon) ahead of the apps; action
        // rows always sit above any fuzzy matches so Enter hits the answer.
        let action_count = items.len();
        items.extend(apps.into_iter().map(ResultItem::App));
        let icons = std::iter::repeat_n(None, action_count)
            .chain(icons)
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
    fn after_confirm(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.input.query.clear();
        self.input.cursor = 0;
        self.input.marked = None;
        self.input.selection = None;
        self.mouse_selecting = false;
        self.mouse_anchor = 0;
        // Re-run the empty query so the next summon opens with the most-used
        // applications instead of an empty bar (the recents seed in
        // `open_launcher_window` only runs once, at window creation).
        self.search(window, cx);
        hide_window(window, cx);
        cx.notify();
    }

    /// Start a mouse-driven selection: place the caret at `index` (collapsing
    /// any prior selection) and anchor the drag there.
    fn begin_mouse_selection(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        self.input.set_cursor(index);
        self.mouse_selecting = true;
        self.mouse_anchor = index;
        cx.notify();
    }

    /// Extend the mouse selection from the anchor to `index`, the caret
    /// following the pointer.
    fn update_mouse_selection(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        self.input.select_anchor_to(self.mouse_anchor, index);
        cx.notify();
    }

    /// End a mouse-driven selection drag.
    fn end_mouse_selection(&mut self) {
        self.mouse_selecting = false;
    }
}

/// A blinking text cursor rendered as a thin vertical bar, using the same
/// on/off cadence as the OS caret.
fn cursor(primary: Hsla) -> impl IntoElement {
    div().w(px(2.0)).h(px(18.0)).bg(primary).with_animation(
        "cursor-blink",
        Animation::new(crate::platform::caret_blink_period()).repeat_synced(),
        |this, delta| this.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
    )
}

/// A transparent strip around the launcher content that acts as the window's
/// drag handle. Only these margins start a drag; the input box in the middle
/// stays interactive (text cursor) instead of dragging the window.
fn drag_strip() -> Div {
    div().window_control_area(gpui::WindowControlArea::Drag)
}
