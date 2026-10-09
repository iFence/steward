//! The launcher window's model: query buffer handling, search, rendering and
//! the shared state that outlives any single window.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    sync::Arc,
};

use global_hotkey::hotkey::HotKey;
use global_hotkey::GlobalHotKeyManager;
use gpui::{
    div, point, prelude::*, px, rgb, size, svg, Animation, AnimationExt, AnyElement, App, Bounds,
    ClipboardItem, DispatchPhase, Div, Element, ElementId, ElementInputHandler, EntityInputHandler,
    FocusHandle, GlobalElementId, Hsla, InspectorElementId, InteractiveElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Subscription,
    UTF16Selection, Window,
};
use gpui_component::ActiveTheme;
use steward_core_engine::MatchTier;
use steward_plugin_host::{PluginHost, RouteHit};
use steward_plugin_registry::{PluginMeta, Registry, ScanReport};
use steward_ui_components::{
    calendar_grid_height, days_in_month, iso_date, month_week_rows, CalendarData, CalendarView,
    ResultItem, ResultList,
};

use crate::config::{
    HideWindow, LAUNCHER_HEIGHT, LAUNCHER_MARGIN, LAUNCHER_WIDTH, MAX_PLUGIN_ROWS, MAX_RESULT_ROWS,
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
/// Width of the file-search page's "back" chip (chevron plus page title), and
/// the gap between it and the query text. The query text starts to the right of
/// it, so every window-x -> character mapping (mouse selection, caret and IME
/// candidate placement) has to add both while that page is open.
const BACK_CHIP_WIDTH: f32 = 104.0;
const BACK_CHIP_GAP: f32 = 8.0;

/// Window-x of the query text's left edge: the launcher's left drag-strip
/// margin plus the input row's `px_3` padding, plus the file-search page's back
/// chip while that page is open.
fn query_text_x(page: LauncherPage) -> f32 {
    let base = LAUNCHER_MARGIN + 12.0;
    match page {
        LauncherPage::Root => base,
        LauncherPage::FileSearch => base + BACK_CHIP_WIDTH + BACK_CHIP_GAP,
    }
}

/// Lucide `chevron-left` icon (24x24, stroke 2, `currentColor`), used by the
/// file-search page's "back" control.
const CHEVRON_LEFT_ICON_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>"#;

/// Lucide `external-link` icon (24x24, stroke 2, `currentColor`), used by the
/// generic "pop out a plugin view" control shown on detachable list panels.
const POP_OUT_ICON_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>"#;

/// Height of the results drop-down for `count` visible rows, capped at
/// `MAX_RESULT_ROWS` so the window stops growing once the list scrolls.
fn result_height(count: usize) -> f32 {
    RESULT_ROW_HEIGHT * count.min(MAX_RESULT_ROWS) as f32
}

/// Shortest gap between two file-index searches, in milliseconds.
///
/// A keystroke burst is answered by re-ranking the app and plugin rows
/// immediately (they are in memory) while the file index catches up at this
/// cadence: fast enough to feel live, slow enough that a typed word does not
/// queue one full index scan per character.
const FILE_SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(80);

/// What a render was built from, for "did anything change?" checks.
/// Compared on every poll tick (10 ms). Cheap to compute and cheap to compare:
/// the query string, what the file index would contribute, and whether the index
/// is still being built (which is what the empty-list hint depends on).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RenderSignature {
    page: LauncherPage,
    query: String,
    file_ready: bool,
    file_building: bool,
    file_records: usize,
    file_hits: Vec<(std::path::PathBuf, i32)>,
}

impl RenderSignature {
    /// Capture the state a render depends on.
    pub(crate) fn capture(app: &StewardApp) -> Self {
        let state = app.state.borrow();
        Self {
            page: app.page,
            query: app.input.query.clone(),
            file_ready: state.file_index.is_ready(),
            file_building: state.file_index.is_building(),
            file_records: state.file_index.records,
            file_hits: state
                .file_hits
                .iter()
                .map(|hit| (hit.path.clone(), hit.score))
                .collect(),
        }
    }
}

/// A result row together with what it takes to rank it against rows of every
/// other kind.
///
/// Icons travel with the row because the merged list is sorted after the icons
/// are resolved, so the two must move together or the icons would belong to the
/// wrong names.
pub(crate) struct RankedRow {
    pub(crate) item: ResultItem,
    pub(crate) icon: Option<Arc<gpui::Image>>,
    pub(crate) rank: RowRank,
}

/// Sorting key for a result row: how well it matches, then how good that match
/// is, then how short and how shallow the name is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RowRank {
    pub(crate) tier: MatchTier,
    /// Within a tier: higher first, so the ordering is reversed before use.
    pub(crate) relevance: i64,
    /// Within a tier: shorter names first (they are usually the better answer).
    pub(crate) name_len: usize,
    /// Stable tie-break so the list does not reshuffle between identical rows.
    pub(crate) path: Vec<u8>,
}

impl RowRank {
    /// Rank for a row whose text is `name`, matched against `needle`.
    ///
    /// `relevance` is a within-tier tie-break (nucleo's fuzzy score for an
    /// application, the file index's own score for a file); it is scaled down so
    /// it can order rows inside a tier but never promote one past a better tier.
    pub(crate) fn new(needle: &str, name: &str, relevance: i64, path: &std::path::Path) -> Self {
        Self {
            tier: steward_core_engine::match_tier(needle, name, path.to_str()),
            relevance: relevance.clamp(-1_000_000, 1_000_000),
            name_len: name.chars().count(),
            path: path.to_string_lossy().to_lowercase().into_bytes(),
        }
    }

    /// Best-first ordering.
    pub(crate) fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.tier
            .cmp(&other.tier)
            .then_with(|| other.relevance.cmp(&self.relevance))
            .then_with(|| self.name_len.cmp(&other.name_len))
            .then_with(|| self.path.cmp(&other.path))
    }
}

/// Order rows best-first for the drop-down.
///
/// Sorting by match quality across *all* kinds is the fix for the launcher
/// looking like it only matches from the first letter: rows used to be appended
/// kind by kind (builtins, plugins, apps, files) and the drop-down shows eight,
/// so any file hit after eight app hits was invisible no matter how exact it was.
fn rank_rows(rows: &mut [RankedRow]) {
    rows.sort_by(|left, right| left.rank.compare(&right.rank));
}

/// Split a plugin command title out of a query for tiering: a plugin that
/// declares `keywords` matches a localized alias, so the title is the best
/// available signal for "how directly did the user ask for this".
fn plugin_row_rank(needle: &str, title: &str, command: &str) -> RowRank {
    let tier = steward_core_engine::match_tier(needle, title, None)
        .min(steward_core_engine::match_tier(needle, command, None));
    RowRank {
        tier,
        relevance: 0,
        name_len: title.chars().count(),
        path: command.as_bytes().to_vec(),
    }
}

/// The file generation the index worker is currently on. Used by the render
/// paths that are not answering a search (a plugin view landing): they render
/// whatever hits are in hand, so "the current generation" is the honest label.
fn current_file_generation(app: &StewardApp) -> u64 {
    app.state.borrow().file_hits_generation
}

/// Secondary text for a file row: the containing folder, so the row answers
/// "where is it". The size is *not* folded in here — the widget renders it as
/// its own right-aligned column, pinning it to the row's edge; appending it to
/// the path is what used to leave "…\Verifier · 120.6 KB" reading as if the size
/// were part of the folder name.
fn file_subtitle(hit: &steward_core_engine::file_index::FileHit) -> String {
    hit.path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The size column for a file row: the formatted byte count, or empty for a
/// folder (which has no meaningful size of its own).
fn file_size(hit: &steward_core_engine::file_index::FileHit) -> String {
    match (hit.is_dir, hit.size) {
        (true, _) | (false, None) => String::new(),
        (false, Some(size)) => human_size(size),
    }
}

/// Byte count for a result row, e.g. `1.4 MB`. Binary units, matching what
/// Explorer reports for the same file.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The launcher's active page: one window, two levels.
///
/// The root page answers applications, plugin commands and the built-in
/// commands; the file-search page (which a keyword on the root page drills
/// into) answers from the file index alone. The query text is shared between
/// them, so the two levels never mix rows: a file can only appear on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LauncherPage {
    #[default]
    Root,
    /// The second-level file search.
    FileSearch,
}

/// Keywords that offer the built-in "File Search" command.
///
/// `fs` is the short form; the full names are accepted in English and Chinese
/// whatever the UI language is, because a keyword is something a user memorizes
/// once (the command row itself is localized like every other row).
const FILE_SEARCH_KEYWORDS: [&str; 3] = ["fs", "file search", "文件搜索"];

/// Parse the file-search keyword out of a root-page query.
///
/// Returns the file query the page should open with: the text after the
/// keyword, so `fs report` drills in already searching `report`, and a bare
/// `fs` opens the page with an empty box. The keyword has to be a whole token
/// (`fsx` and `files` are names, not the command) and only a leading one
/// counts.
fn parse_file_search_command(query: &str) -> Option<&str> {
    let trimmed = query.trim_start();
    for keyword in FILE_SEARCH_KEYWORDS {
        let Some(head) = trimmed.get(..keyword.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(keyword) {
            continue;
        }
        let rest = &trimmed[keyword.len()..];
        if rest.is_empty() {
            return Some("");
        }
        if rest.starts_with(char::is_whitespace) {
            return Some(rest.trim_start());
        }
    }
    None
}

/// Which row the highlight lands on after a keystroke changed the row list.
///
/// The file-search page opens on its first row. The root page opens on the
/// "File Search" command only when the query just produced it — that is the row
/// the user asked for — while the home page pins the same command on top for
/// discoverability but still opens on the most-used application, so "summon +
/// Enter" keeps launching it.
fn default_selection(page: LauncherPage, query: &str, offers_file_search: bool) -> usize {
    match page {
        LauncherPage::FileSearch => 0,
        LauncherPage::Root if offers_file_search && !query.trim().is_empty() => 0,
        LauncherPage::Root if offers_file_search => 1,
        LauncherPage::Root => 0,
    }
}

/// Whether a file-page query is a search at all.
///
/// An empty (or whitespace-only) query never reaches the index: it answers `""`
/// with its first records in path order, which is a directory listing rather
/// than a search. The page shows a hint instead. Any non-whitespace term is a
/// real search.
fn is_file_search(query: &str) -> bool {
    !query.trim().is_empty()
}

/// Total launcher window height for a given number of visible result rows:
/// the input bar plus the top/bottom drag margins plus the (possibly
/// scroll-capped) drop-down. The margins are part of the window, so they must
/// be counted or the flex column squeezes the drop-down's last row.
fn launcher_height(result_count: usize) -> f32 {
    LAUNCHER_HEIGHT + 2.0 * LAUNCHER_MARGIN + result_height(result_count)
}

/// Total launcher window height when a plugin calendar view is active: the
/// input bar plus the month grid sized to the month's actual week rows (no
/// results list).
fn calendar_height(data: &CalendarData) -> f32 {
    let rows = month_week_rows(data.year, data.month, data.start_of_week);
    LAUNCHER_HEIGHT + 2.0 * LAUNCHER_MARGIN + calendar_grid_height(rows)
}

/// Height of the drop-down area a plugin `ui` tree occupies when rendered
/// inline in the launcher. Rich interactive trees are better popped out; the
/// inline rendering is for compact, mostly-display views.
const UI_INLINE_HEIGHT: f32 = 320.0;

/// Total window height while an inline `ui` tree is displayed.
fn ui_inline_height() -> f32 {
    LAUNCHER_HEIGHT + 2.0 * LAUNCHER_MARGIN + UI_INLINE_HEIGHT
}

/// A plugin `calendar` view currently displayed in the launcher, plus the
/// plugin that produced it (needed for `item.invoke` on day selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveCalendar {
    pub data: CalendarData,
    pub plugin_id: String,
    /// The command that produced this view (routing key for the detach
    /// registry, so re-opening a detached window reuses the same key).
    pub command: String,
    /// Whether the command declared `detachable` (controls the detach/dock
    /// affordance shown in the calendar header).
    pub detachable: bool,
}

pub(crate) struct StewardApp {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) input: SearchInput,
    /// Which level of the launcher is showing. The root page never lists files
    /// and the file-search page never lists applications or plugins, so the two
    /// can share one query buffer without mixing rows.
    pub(crate) page: LauncherPage,
    pub(crate) i18n: Rc<crate::i18n::Localization>,
    /// Shared search index, rebuilt at startup from a scan / cache.
    pub(crate) engine: Rc<RefCell<steward_core_engine::Engine>>,
    /// SQLite cache plus usage-frequency tracking.
    pub(crate) storage: Rc<RefCell<steward_storage::Storage>>,
    /// Current result rows, mirrored for the `on_confirm` callback to resolve
    /// the selected index into an app path (or a calculator result to copy).
    pub(crate) last_results: Rc<RefCell<Vec<ResultItem>>>,
    pub(crate) results: ResultList,
    /// Month calendar grid, shown instead of the results list when the current
    /// query's plugin view is a `calendar` view.
    pub(crate) calendar: CalendarView,
    /// Keyboard-selected date inside the calendar grid (ISO `YYYY-MM-DD`).
    pub(crate) calendar_selected: String,
    /// Base rows of the current query (builtin actions + app and file matches),
    /// ranked together and each carrying its own icon, kept separate from the
    /// plugin rows so a late plugin response can be re-merged without re-running
    /// the search.
    pub(crate) base_rows: Vec<RankedRow>,
    /// Number of leading builtin rows (calculator/link) inside `base_rows`;
    /// plugin rows are inserted after the builtins' tier but never above them,
    /// so Enter still hits a calculator answer first.
    pub(crate) builtin_count: usize,
    /// The query text the current rows were ranked against, so a re-merge ranks
    /// plugin rows with the same needle.
    pub(crate) ranked_query: String,
    /// What the last render was built from: the query, the file rows and the
    /// file-index state. The poll task compares against this so a tick that
    /// changes nothing does not re-run the search — re-rendering on a timer reset
    /// the highlighted row ten times a second, which both flickered the window and
    /// made the arrow keys look broken.
    pub(crate) rendered: RefCell<RenderSignature>,
    /// When the last file-index search was dispatched, so a burst of keystrokes
    /// does not queue one search per character. The generation the worker gave
    /// that request lives in `LauncherState::file_search_generation`, because it
    /// is what the *reply* has to be matched against.
    pub(crate) file_search_at: Cell<Option<std::time::Instant>>,
    /// The input the displayed rows were actually produced for.
    ///
    /// The file index keeps the previous rows on screen while a new search
    /// runs (blanking them flashed the drop-down on every keystroke), so "are
    /// these rows still the answer to what is in the box" has to be tracked
    /// explicitly: the producer records it, and `render_merged`
    /// re-enables confirmation only when it matches the current input. Without it
    /// a stale row could be launched, and — the bug this replaced — a *fresh* row
    /// could be refused, because the flag was only ever set on the typing path.
    ///
    /// It is deliberately **not** written by `render_merged`: a render happens
    /// while results are still in flight, so "I just rendered" is not evidence
    /// that the rows answer what the user has typed.
    pub(crate) results_query: String,
    /// Result count is published here so the tray/hotkey path can size the
    /// window when it is summoned.
    pub(crate) state: Rc<RefCell<LauncherState>>,
    /// The unique detachable `list` plugin view currently displayed, if any
    /// (`(plugin_id, command)`). Drives the launcher's "pop out" button; only
    /// set when the active query has exactly one detachable list panel so the
    /// target is unambiguous.
    pub(crate) detachable_list_target: Option<(String, String)>,
    /// Inline Virtual UI Tree for the current query, when the active plugin
    /// view is a `ui` tree and is not popped out into its own window.
    pub(crate) ui_view: Option<gpui::Entity<steward_ui_components::virtual_tree::VirtualTreeView>>,
    /// The `(plugin_id, command)` [`StewardApp::ui_view`] belongs to.
    pub(crate) ui_target: Option<(String, String)>,
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
/// A finished background plugin scan: the reconcile report plus the plugin
/// set to (re)load into the host.
type PluginScan = (ScanReport, Vec<PluginMeta>);

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
    /// Number of rows the drop-down last rendered, so the bar is sized for the
    /// results on screen when it is summoned.
    pub(crate) result_count: std::cell::Cell<usize>,
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
    /// Plugin row icons (inline SVG from the manifest cache, wrapped as
    /// `gpui::Image`), keyed by plugin id. Refreshed at startup and after
    /// every plugin scan, so plugin rows render an icon like app rows.
    pub(crate) plugin_icons: RefCell<HashMap<String, Option<Arc<gpui::Image>>>>,
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
    pub(crate) scan_rx:
        RefCell<Option<crossbeam_channel::Receiver<Vec<steward_core_engine::AppEntry>>>>,
    /// Live app-index maintenance: decides when to re-run the scanner while the
    /// app runs (Start Menu change notifications plus a reconcile timer that
    /// covers UWP/Store apps). `None` until [`Self::start_app_watch`] runs at
    /// boot.
    pub(crate) app_watch: RefCell<Option<crate::app_index::AppIndexWatcher>>,
    /// Plugin host: runtime processes, trigger routing, command invocation.
    pub(crate) plugin_host: Rc<RefCell<PluginHost>>,
    /// Plugin metadata cache (SQLite) plus the plugin root to scan.
    pub(crate) plugin_registry: Rc<RefCell<Registry>>,
    /// Pending background plugin-scan results, drained by the foreground poll.
    pub(crate) plugin_scan_rx: RefCell<Option<crossbeam_channel::Receiver<PluginScan>>>,
    /// Query generation for plugin invocations: bumped on every search so a
    /// stale response is dropped instead of merged into newer results.
    pub(crate) plugin_gen: Cell<u64>,
    /// Plugin route hits for the current query (aligned with `plugin_views`).
    pub(crate) plugin_hits: RefCell<Vec<RouteHit>>,
    /// Views received for each hit; `None` until the response (or an error)
    /// lands for this query.
    pub(crate) plugin_views: RefCell<Vec<Option<serde_json::Value>>>,
    /// Plugin commands still running for the current query generation, keyed
    /// by `(plugin_id, command)`. Populated by `search` when it dispatches a
    /// command and cleared by the poll task when its `CommandResult` lands
    /// (success or error), so the launcher can show a transient "running"
    /// placeholder instead of leaving a gap while a cold plugin lazy-loads or
    /// an async `command()` drains its micro-tasks.
    pub(crate) plugin_pending: RefCell<HashSet<(String, String)>>,
    /// Generation for `search.query` invocations: bumped on every keystroke so
    /// a stale search response is dropped instead of overwriting newer results.
    pub(crate) search_gen: Cell<u64>,
    /// Latest `search.query` result per `(plugin_id, command)`, so a `search`
    /// view renders its results in the launcher's drop-down (or a detached
    /// panel reads it on open). Cleared when the command's view resets.
    pub(crate) plugin_search_results: RefCell<HashMap<(String, String), serde_json::Value>>,
    /// Bumped every time the launcher window is shown, so the root's entrance
    /// animation restarts (the `with_animation` key is derived from it).
    pub(crate) show_epoch: Cell<u64>,
    /// The active calendar view (from the first calendar-typed plugin view of
    /// the current query), if any.
    pub(crate) plugin_calendar: RefCell<Option<ActiveCalendar>>,
    /// Whether an inline `ui` tree is currently occupying the drop-down, so
    /// the summon path sizes the window like the calendar path does.
    pub(crate) plugin_ui_inline: Cell<bool>,
    /// Open plugin-view windows, keyed by `(plugin_id, command)`. Each entry
    /// is an independent PopUp that lives outside the launcher (never toggled
    /// by the summon hotkey and never hidden on launcher blur). Generic: any
    /// command whose manifest sets `detachable` can pop a view here.
    pub(crate) panel_view_windows: RefCell<HashMap<(String, String), gpui::AnyWindowHandle>>,
    /// Plugin panels docked in the workspace window, keyed by
    /// `(plugin_id, command)`. A docked panel counts as "popped out": the
    /// launcher shows the command row rather than the inline view.
    pub(crate) workspace_panels:
        RefCell<HashMap<(String, String), gpui::Entity<crate::plugin_workspace::PluginDockPanel>>>,
    /// The plugin workspace window, when open.
    pub(crate) workspace_window: RefCell<Option<gpui::AnyWindowHandle>>,
    /// The workspace's dock area, so panels can be added and removed from
    /// outside the workspace view.
    pub(crate) workspace_dock: RefCell<Option<gpui::Entity<gpui_component::dock::DockArea>>>,
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
    /// Receiver for host-side clipboard history snapshots, drained by the
    /// foreground poll task and forwarded to the plugin host.
    pub(crate) clipboard_rx:
        RefCell<Option<crossbeam_channel::Receiver<Vec<steward_ipc_protocol::ClipboardEntry>>>>,
    /// Full-disk file index: persisted snapshot, background build, USN catch-up
    /// and off-thread search.
    pub(crate) file_index: crate::file_index::FileIndex,
    /// Hits from the most recent file search, merged into the drop-down.
    pub(crate) file_hits: Vec<steward_core_engine::file_index::FileHit>,
    /// The generation whose hits are in [`Self::file_hits`] right now — not the
    /// generation of the last request. Emptying the hits counts as a new set, so
    /// this can be trusted to say "these are the rows for request N".
    pub(crate) file_hits_generation: u64,
    /// How long the last file search took, for the diagnostics line.
    pub(crate) file_search_ms: u128,
    /// The index worker's generation for the last *dispatched* search. The reply
    /// carries it back, and it is what makes the rows confirmable — see the gate
    /// at the end of `render_merged`.
    pub(crate) file_search_generation: u64,
    /// The tray's status line, so the poll task can report file-index progress
    /// where the user can see it without summoning the launcher. Empty until the
    /// tray icon exists (and on platforms without one).
    pub(crate) tray_status: Rc<RefCell<Option<crate::tray::TrayStatusItem>>>,
    /// Keeps the host-side clipboard watcher alive (its thread owns a private
    /// SQLite connection and the arboard clipboard).
    pub(crate) _clipboard_watcher: Option<crate::clipboard_history::ClipboardWatcher>,
}

impl LauncherState {
    /// Width of the launcher bar in logical px.
    pub(crate) fn width(&self) -> f32 {
        LAUNCHER_WIDTH
    }

    /// Total launcher window height for the current result count: the input
    /// bar plus the result drop-down.
    pub(crate) fn height(&self) -> f32 {
        let calendar = self.plugin_calendar.borrow();
        if let Some(active) = calendar.as_ref() {
            if !self.is_active_panel_detached() {
                return calendar_height(&active.data);
            }
        }
        if self.plugin_ui_inline.get() {
            return ui_inline_height();
        }
        launcher_height(self.result_count.get())
    }

    /// Whether a plugin view is currently popped out into its own window.
    pub(crate) fn plugin_window_open(&self, plugin_id: &str, command: &str) -> bool {
        self.panel_view_windows
            .borrow()
            .contains_key(&(plugin_id.to_string(), command.to_string()))
            || crate::plugin_workspace::is_panel_open(self, plugin_id, command)
    }

    /// The open detached window handle for a plugin view, if any.
    pub(crate) fn plugin_window(
        &self,
        plugin_id: &str,
        command: &str,
    ) -> Option<gpui::AnyWindowHandle> {
        self.panel_view_windows
            .borrow()
            .get(&(plugin_id.to_string(), command.to_string()))
            .cloned()
            .or_else(|| {
                if crate::plugin_workspace::is_panel_open(self, plugin_id, command) {
                    *self.workspace_window.borrow()
                } else {
                    None
                }
            })
    }

    /// Whether the active calendar panel is detached into its own window.
    pub(crate) fn is_active_panel_detached(&self) -> bool {
        self.plugin_calendar
            .borrow()
            .as_ref()
            .is_some_and(|active| self.plugin_window_open(&active.plugin_id, &active.command))
    }

    /// The raw plugin `view` for a given `(plugin_id, command)` hit, if its
    /// response has landed for the current query. Used to hand the view to a
    /// detached window host.
    pub(crate) fn plugin_view(&self, plugin_id: &str, command: &str) -> Option<serde_json::Value> {
        let hits = self.plugin_hits.borrow();
        let views = self.plugin_views.borrow();
        hits.iter()
            .zip(views.iter())
            .find(|(hit, _)| hit.plugin_id == plugin_id && hit.command == command)
            .and_then(|(_, view)| view.clone())
    }

    /// Seed the search index from the SQLite cache, then always start a
    /// background reconcile whose results are applied by
    /// [`Self::apply_scan_results`]. Never blocks the caller: the scan runs on a
    /// worker thread and both event loops drain the result channel.
    ///
    /// The cache makes the cold start searchable immediately; the reconcile is
    /// what catches installs/uninstalls made while Steward was closed.
    pub(crate) fn ensure_app_index(&self) {
        let cached = self.storage.borrow().cached_apps().unwrap_or_default();
        if !cached.is_empty() {
            self.engine.borrow_mut().set_entries(cached);
        }
        self.spawn_app_scan();
    }

    /// Start the live app-index watch: Start Menu change notifications for
    /// desktop installs/uninstalls, plus the reconcile timer that covers
    /// UWP/Store apps. Called once at boot.
    pub(crate) fn start_app_watch(&self) {
        if self.app_watch.borrow().is_some() {
            return;
        }
        *self.app_watch.borrow_mut() = Some(crate::app_index::AppIndexWatcher::start(
            std::time::Instant::now(),
        ));
    }

    /// Start a background app scan when one is not already running. Returns
    /// whether a scan was started.
    pub(crate) fn spawn_app_scan(&self) -> bool {
        if self.scan_rx.borrow().is_some() {
            return false;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let apps = steward_core_engine::platform_scanner().scan();
            let _ = tx.send(apps);
        });
        *self.scan_rx.borrow_mut() = Some(rx);
        true
    }

    /// Apply a finished background scan: persist the cache and rebuild the
    /// index. Runs on the main thread from the GPUI foreground poll task.
    ///
    /// Returns whether the app set actually changed, so the caller can re-run
    /// the visible query. A reconcile that found nothing new is a no-op.
    pub(crate) fn apply_scan_results(&self) -> bool {
        let Some(rx) = self.scan_rx.borrow().clone() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(apps) if !apps.is_empty() => {
                    let current = self.engine.borrow().entries().to_vec();
                    if !crate::app_index::entries_differ(&current, &apps) {
                        continue;
                    }
                    if let Err(error) = self.storage.borrow_mut().mark_seen(&apps) {
                        eprintln!("failed to persist scanned apps: {error:#}");
                    }
                    self.engine.borrow_mut().set_entries(apps);
                    changed = true;
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
        changed
    }

    /// Ask the file index for a build, or for a journal catch-up when a snapshot
    /// was already loaded and the index is usable.
    pub(crate) fn start_file_index(&mut self) {
        if !self.file_index.is_ready() {
            self.file_index.request_build();
            return;
        }
        if self.file_index.supports_journal() {
            // The snapshot is searchable already; a catch-up pass folds in
            // whatever each volume's USN journal recorded since it was written,
            // including while Steward was closed.
            self.file_index.request_catch_up();
        } else {
            // A directory-walk index has no journal: the only way to pick up
            // changes made while Steward was closed is a fresh background pass.
            // The live watcher handles everything from here on, and the loaded
            // snapshot stays searchable throughout the rebuild.
            self.file_index.request_build();
        }
    }

    /// Seed the plugin host from the metadata cache and start a background
    /// reconcile. Cold start reads SQLite only (never a full file walk); the
    /// scan runs on a worker thread and its result is applied by
    /// [`Self::apply_plugin_scan`].
    pub(crate) fn ensure_plugin_index(&self) {
        let cached = self
            .plugin_registry
            .borrow()
            .cached_plugins()
            .unwrap_or_default();
        if !cached.is_empty() {
            self.refresh_plugin_icons(&cached);
            if let Err(error) = self.plugin_host.borrow_mut().set_plugins(&cached) {
                eprintln!("failed to start the plugin runtime: {error:#}");
            }
        }
        if self.plugin_scan_rx.borrow().is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        // The worker opens its own connection to the same cache (rusqlite
        // `Connection` is `Send`, `Rc<RefCell<...>>` is not); WAL lets both
        // connections coexist.
        let (data_dir, plugins_root) = {
            let registry = self.plugin_registry.borrow();
            (
                registry.data_dir().to_path_buf(),
                registry.plugins_root().to_path_buf(),
            )
        };
        std::thread::spawn(move || {
            let Ok(mut registry) = Registry::open_with_root(&data_dir, &plugins_root) else {
                return;
            };
            let report = registry.scan().unwrap_or_default();
            let metas = registry.cached_plugins().unwrap_or_default();
            let _ = tx.send((report, metas));
        });
        *self.plugin_scan_rx.borrow_mut() = Some(rx);
    }

    /// Apply a finished plugin scan: (re)load changed plugins into the host.
    /// Runs on the main thread from the GPUI foreground poll task.
    pub(crate) fn apply_plugin_scan(&self) {
        let Some(rx) = self.plugin_scan_rx.borrow().clone() else {
            return;
        };
        match rx.try_recv() {
            Ok((report, metas)) => {
                self.refresh_plugin_icons(&metas);
                if let Err(error) = self.plugin_host.borrow_mut().set_plugins(&metas) {
                    eprintln!("failed to load plugins after scan: {error:#}");
                }
                if report.plugins == 0 && report.failed.is_empty() {
                    eprintln!("no plugins installed (STEWARD_PLUGINS_DIR may point elsewhere)");
                }
                *self.plugin_scan_rx.borrow_mut() = None;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                *self.plugin_scan_rx.borrow_mut() = None;
            }
        }
    }

    /// Rebuild the plugin icon map from the current plugin set. Each manifest
    /// icon is an inline SVG document, wrapped as `ImageFormat::Svg` so the
    /// results list renders it through the same `img()` path as app icons.
    pub(crate) fn refresh_plugin_icons(&self, metas: &[PluginMeta]) {
        let mut icons = HashMap::with_capacity(metas.len());
        for meta in metas {
            icons.insert(
                meta.manifest.id.clone(),
                meta.icon.as_ref().map(|svg| {
                    Arc::new(gpui::Image::from_bytes(
                        gpui::ImageFormat::Svg,
                        svg.as_bytes().to_vec(),
                    ))
                }),
            );
        }
        *self.plugin_icons.borrow_mut() = icons;
    }

    /// Build the rows for the current query's plugin views: one `Command` entry
    /// row per plugin hit. Confirming a row opens the plugin's view in its own
    /// independent application window, so every plugin behaves like a launched
    /// app (no list-item flattening and no inline calendar grid). Views arrive
    /// asynchronously; this is called on every merge. `command_label` is the
    /// localized "Command" tag shown on those rows. `calendars` is kept as an
    /// empty vec for call-site compatibility (inline calendar is retired by
    /// the application-window model).
    pub(crate) fn plugin_rows_and_calendar_rows(
        &self,
        command_label: &str,
    ) -> (Vec<ResultItem>, Vec<ResultItem>) {
        let hits = self.plugin_hits.borrow();
        let views = self.plugin_views.borrow();
        let mut rows = Vec::new();
        let calendars = Vec::new();
        for (hit, view) in hits.iter().zip(views.iter()) {
            if view.is_none() {
                continue;
            }
            if rows.len() >= MAX_PLUGIN_ROWS {
                return (rows, calendars);
            }
            rows.push(ResultItem::Command {
                plugin_id: hit.plugin_id.clone(),
                command: hit.command.clone(),
                title: hit.title.clone(),
                subtitle: command_label.to_string(),
            });
        }
        (rows, calendars)
    }
}

/// Extract the list items from a plugin command result. The runtime wraps the
/// view in `{ "view": ... }` (see `command.invoke` in the service loop), so
/// both the wrapped response and a bare view are accepted.
pub(crate) fn plugin_view_items(view: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    let view = view.get("view").unwrap_or(view);
    if view.get("type").and_then(|kind| kind.as_str()) != Some("list") {
        return None;
    }
    view.get("items").and_then(|items| items.as_array())
}

/// Whether a plugin view is a `detail` / `form` (a panel-hosting view rather
/// than a row-producing list or an inline calendar grid).
pub(crate) fn is_detail_or_form_view(view: &serde_json::Value) -> bool {
    let view = view.get("view").unwrap_or(view);
    matches!(
        view.get("type").and_then(|kind| kind.as_str()),
        Some("detail") | Some("form") | Some("ui")
    )
}

/// Whether a plugin view is hostable in a detached panel window: the panel
/// dispatches on `list` / `grid` / `search` / `detail` / `form`.
pub(crate) fn is_panel_view(view: &serde_json::Value) -> bool {
    let view = view.get("view").unwrap_or(view);
    matches!(
        view.get("type").and_then(|kind| kind.as_str()),
        Some("list" | "grid" | "search" | "detail" | "form" | "ui")
    )
}

/// Build the event sink an inline `ui` tree uses: every element event is
/// forwarded to the plugin's isolate as a `view.invoke`.
fn launcher_event_sink(
    state: Rc<RefCell<LauncherState>>,
    plugin_id: String,
    command: String,
) -> steward_ui_components::virtual_tree::EventSink {
    Rc::new(move |callback_id, node_id, event, value| {
        let Some(callback_id) = callback_id else {
            return;
        };
        let mut payload = serde_json::json!({
            "type": event.as_str(),
            "node_id": node_id,
        });
        if let Some(value) = value {
            payload["value"] = serde_json::Value::String(value);
        }
        let host = state.borrow().plugin_host.clone();
        if host
            .borrow_mut()
            .invoke_view(&plugin_id, &command, callback_id, &payload)
            .is_none()
        {
            eprintln!("[steward] plugin {plugin_id} not ready for view.invoke");
        }
    })
}

/// Parse a plugin `calendar` view (wrapped or bare) into the display data.
pub(crate) fn parse_calendar_view(
    view: &serde_json::Value,
    plugin_id: &str,
    command: &str,
    detachable: bool,
) -> Option<ActiveCalendar> {
    let view = view.get("view").unwrap_or(view);
    if view.get("type").and_then(|kind| kind.as_str()) != Some("calendar") {
        return None;
    }
    let year = view.get("year")?.as_i64()? as i32;
    let month = view.get("month")?.as_i64()? as u32;
    let today = view.get("today")?.as_str()?.to_string();
    let start_of_week = view
        .get("startOfWeek")
        .and_then(|value| value.as_u64())
        .unwrap_or(1) as u8;
    let selected = view
        .get("selected")
        .and_then(|value| value.as_str())
        .unwrap_or(&today)
        .to_string();
    Some(ActiveCalendar {
        data: CalendarData {
            year,
            month: month.clamp(1, 12),
            today,
            selected,
            start_of_week: start_of_week % 7,
        },
        plugin_id: plugin_id.to_string(),
        command: command.to_string(),
        detachable,
    })
}

/// Parse `YYYY-MM-DD` into `(year, month, day)`.
pub(crate) fn parse_iso_date(iso: &str) -> Option<(i32, u32, u32)> {
    let bytes = iso.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 || index == 7 {
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
    }
    let year = iso[..4].parse().ok()?;
    let month = iso[5..7].parse().ok()?;
    let day = iso[8..10].parse().ok()?;
    Some((year, month, day))
}

/// Localized month label for the calendar header, e.g. "August 2026" or
/// "2026 年 8 月".
pub(crate) fn calendar_month_label(language: &str, year: i32, month: u32) -> String {
    if language.starts_with("zh") {
        format!("{year} 年 {month} 月")
    } else {
        const MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let name = MONTHS
            .get(month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("?");
        format!("{name} {year}")
    }
}

/// Localized weekday labels ordered from `start_of_week` (0 = Sunday-first,
/// 1 = Monday-first).
pub(crate) fn calendar_weekday_labels(language: &str, start_of_week: u8) -> [String; 7] {
    // Canonical Sunday-first order; rotated by `start_of_week` below.
    let base: [&str; 7] = if language.starts_with("zh") {
        ["日", "一", "二", "三", "四", "五", "六"]
    } else {
        ["S", "M", "T", "W", "T", "F", "S"]
    };
    let start = (start_of_week % 7) as usize;
    let mut labels = std::array::from_fn(|_| String::new());
    for (index, label) in labels.iter_mut().enumerate() {
        *label = base[(start + index) % 7].to_string();
    }
    labels
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
        let input = &self.input;
        let range = match &input.selection {
            Some(range) => {
                let start = input.char_to_utf16(range.start);
                let end = input.char_to_utf16(range.end);
                start..end
            }
            None => {
                let caret = input.char_to_utf16(input.cursor);
                caret..caret
            }
        };
        let reversed = input
            .selection
            .as_ref()
            .is_some_and(|range| input.cursor == range.start);
        Some(UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Range<usize>> {
        let input = &self.input;
        input
            .marked
            .as_ref()
            .map(|range| input.char_to_utf16(range.start)..input.char_to_utf16(range.end))
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<String> {
        let input = &self.input;
        let chars = input.utf16_to_chars(range_utf16.clone())?;
        *adjusted_range = Some(input.char_to_utf16(chars.start)..input.char_to_utf16(chars.end));
        Some(
            input.query[input.byte_at_char(chars.start)..input.byte_at_char(chars.end)].to_string(),
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
        self.search_unless_composing(window, cx);
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
        self.search_unless_composing(window, cx);
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.input.marked = None;
        // A composition can end without a commit callback (an IME that cancels,
        // or a platform that only unmarks). If the query moved on since the last
        // search, catch up now instead of leaving the rows stale.
        if self.rendered.borrow().query != self.input.query {
            self.search(window, cx);
            return;
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Approximate the caret position for the IME candidate window: the
        // query text's left edge (which moves right on the file-search page,
        // where the back chip sits before the text) plus an estimated glyph
        // width per character.
        let chars = self.input.utf16_to_chars(range_utf16).unwrap_or(0..0);
        let x = query_text_x(self.page) + GLYPH_WIDTH * chars.start as f32;
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
        let relative = point.x - px(query_text_x(self.page));
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
        let input = &mut self.input;
        if range_utf16.start == range_utf16.end {
            if let Some(index) = input.utf16_to_char_index(range_utf16.start) {
                input.set_cursor(index);
            }
        } else if let Some(range) = input.utf16_to_chars(range_utf16) {
            input.set_selection(range);
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
/// `text_x` is the query text's left edge inside the window (see
/// [`query_text_x`]) — the launcher hand-renders its text, so there are no
/// metrics to ask for.
fn char_index_at_x(bounds: Bounds<Pixels>, query_len: usize, text_x: f32, x: Pixels) -> usize {
    let relative = x - bounds.origin.x - px(text_x);
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
                let index = char_index_at_x(
                    input_bounds,
                    app.input.char_count(),
                    query_text_x(app.page),
                    event.position.x,
                );
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
                let index = char_index_at_x(
                    input_bounds,
                    app.input.char_count(),
                    query_text_x(app.page),
                    event.position.x,
                );
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
        let state = self.state.borrow();
        let calendar_active =
            state.plugin_calendar.borrow().is_some() && !state.is_active_panel_detached();
        let ui_active = self.ui_view.is_some() && !calendar_active;
        // The month grid's height follows the actual number of week rows the
        // displayed month spans (4..=6), so a short month makes the bar shorter
        // instead of leaving an empty six-row grid.
        let calendar_grid_h = state
            .plugin_calendar
            .borrow()
            .as_ref()
            .map(|active| {
                calendar_grid_height(month_week_rows(
                    active.data.year,
                    active.data.month,
                    active.data.start_of_week,
                ))
            })
            .unwrap_or(0.0);
        let primary = cx.theme().primary;
        let root = div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &HideWindow, window, cx| {
                this.dismiss_launcher(window, cx);
            }))
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
                            .child(self.back_control(cx))
                            .child(
                                div()
                                    .flex_1()
                                    .child(
                                        if self.input.query.is_empty()
                                            && self.input.marked.is_none()
                                        {
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
                                                        .child(self.i18n.translate(
                                                            self.search_placeholder(),
                                                        )),
                                                )
                                        } else {
                                            self.render_query_text(primary)
                                        },
                                    ),
                            )
                            .child(self.detach_control(cx)),
                    )
                    .child(drag_strip().w(px(LAUNCHER_MARGIN))),
            )
            .when(calendar_active, |this| {
                // A plugin calendar view replaces the results drop-down: the
                // same separator, then the month grid pinned to its own
                // height.
                this.child(div().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.10)))
                    .child(
                        div()
                            .h(px(calendar_grid_h))
                            .mx(px(LAUNCHER_MARGIN))
                            .child(self.calendar.render(calendar_grid_h, cx)),
                    )
            })
            .when(ui_active, |this| {
                let Some(ui) = self.ui_view.clone() else {
                    return this;
                };
                this.child(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(rgb(0xffffff).opacity(0.10)),
                )
                .child(
                    div()
                        .h(px(UI_INLINE_HEIGHT))
                        .mx(px(LAUNCHER_MARGIN))
                        .child(ui),
                )
            })
            .when(!calendar_active && !ui_active && result_count > 0, |this| {
                // A subtle light hairline separates the search box from the
                // results list (Tinycast's `separator`, white 0.10); it only
                // shows while the drop-down is open.
                let drop_height = result_height(result_count);
                this.child(div().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.10)))
                    // Pin the drop-down to exactly its result height so it never
                    // grows into the input bar. It spans the full launcher width
                    // (no side margin): the selected row's highlight then runs
                    // edge to edge instead of stopping short of the rounded
                    // window corners with a gap on each side. The rows keep their
                    // own inner padding, so their text still lines up with the
                    // query above them.
                    .child(
                        div().h(px(drop_height)).child(self.results.render(
                            drop_height,
                            adaptive_selection_wash(self.state.borrow().scrim_alpha),
                            cx,
                        )),
                    )
            })
            .child(drag_strip().h(px(LAUNCHER_MARGIN)))
            // First-party overlay: plugin toasts (absolute, out of flow).
            .child(crate::overlay::render_toasts(cx));

        // One-shot entrance fade: the launcher eases from transparent to its
        // frosted-glass scrim on every summon (the key includes the show epoch,
        // so a fresh animation starts each time the bar appears). At the end
        // the element holds at its normal opacity; the caret blink below stays
        // independent.
        let root = root.with_animation(
            ElementId::from(format!(
                "launcher-entry-{}",
                self.state.borrow().show_epoch.get()
            )),
            Animation::new(std::time::Duration::from_millis(140)),
            |this, delta| this.opacity(delta),
        );

        LauncherInputElement {
            child: root.into_any_element(),
            focus_handle: self.focus_handle.clone(),
            view: cx.entity(),
            input_bounds: Bounds::default(),
        }
    }
}

impl StewardApp {
    /// Placeholder for the query box: the page decides what the box searches.
    fn search_placeholder(&self) -> &'static str {
        match self.page {
            LauncherPage::Root => "search-placeholder",
            LauncherPage::FileSearch => "file-search-placeholder",
        }
    }

    fn dismiss_launcher(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        hide_window(window, cx);
    }

    /// The file-search page's "back" control, rendered at the left of the input
    /// bar: a chevron plus the page title. Clicking it leaves the page, exactly
    /// like Escape. Only that page renders it, and its width is fixed (the
    /// label truncates instead of stretching) because the query text's x offset
    /// — and with it the caret, the mouse mapping and the IME candidate window —
    /// is computed from [`BACK_CHIP_WIDTH`], not measured.
    fn back_control(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        if self.page != LauncherPage::FileSearch {
            return div().into_any_element();
        }
        div()
            .id(ElementId::from("file-search-back"))
            .flex()
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .w(px(BACK_CHIP_WIDTH))
            .mr(px(BACK_CHIP_GAP))
            .h(px(22.0))
            .overflow_hidden()
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(0xffffff).opacity(0.06))
            .hover(|style| style.bg(rgb(steward_ui_components::palette::HOVER).opacity(0.08)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.leave_file_search(window, cx);
            }))
            .child(
                svg()
                    .data(CHEVRON_LEFT_ICON_SVG)
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(14.0))
                    .text_color(rgb(steward_ui_components::palette::MUTED_FOREGROUND)),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(rgb(steward_ui_components::palette::MUTED_FOREGROUND))
                    .child(self.i18n.translate("file-search-title").to_owned()),
            )
            .into_any_element()
    }

    /// The launcher's generic "pop out" control for the currently displayed
    /// detachable `list` plugin view. Rendered only when [`Self::detachable_list_target`]
    /// is set (exactly one detachable list panel), so the target is
    /// unambiguous; clicking opens the view in its own independent window.
    fn detach_control(&self, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let Some(target) = self.detachable_list_target.clone() else {
            return div().into_any_element();
        };
        let target_for_cb = target.clone();
        div()
            .id(ElementId::from("detach-list-control"))
            .flex()
            .items_center()
            .justify_center()
            .h(px(22.0))
            .px_2()
            .rounded_full()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.08))
            .hover(|style| style.bg(rgb(steward_ui_components::palette::HOVER).opacity(0.05)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let state = this.state.clone();
                let i18n = this.i18n.clone();
                let view = state
                    .borrow()
                    .plugin_view(&target_for_cb.0, &target_for_cb.1);
                if let Some(view) = view {
                    let _ = crate::plugin_panel_window::open_plugin_panel(
                        &state,
                        i18n,
                        target_for_cb.0.clone(),
                        target_for_cb.1.clone(),
                        view,
                        true,
                        cx,
                    );
                }
            }))
            .child(
                svg()
                    .data(POP_OUT_ICON_SVG)
                    .w(px(14.0))
                    .h(px(14.0))
                    .text_color(rgb(steward_ui_components::palette::MUTED_FOREGROUND)),
            )
            .into_any_element()
    }

    /// Render the query text: leading text, the active IME composition
    /// (underlined), the active text selection (washed), the caret and the
    /// trailing text.
    fn render_query_text(&self, primary: Hsla) -> Div {
        let input = &self.input;
        let query = &input.query;
        let mut children: Vec<AnyElement> = Vec::new();

        // The active IME composition owns the rendering (underlined text plus
        // caret); a separate selection cannot coexist with a composition.
        if let Some(range) = &input.marked {
            let start = input.byte_at_char(range.start);
            let end = input.byte_at_char(range.end);
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

        if let Some(range) = input.selection.clone() {
            // The caret sits at the selection head; the selected span is
            // painted with the same adaptive white wash as the result rows.
            let sel_start = input.byte_at_char(range.start);
            let sel_end = input.byte_at_char(range.end);
            let caret = input.byte_at_char(input.cursor);
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
            let caret = input.byte_at_char(input.cursor);
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

        // A plugin calendar view owns the arrow keys and Enter while it is
        // shown inline (not popped out): navigate the selected day or confirm
        // (copy) it. Typing still edits the query, which returns the launcher
        // to list mode until the next view lands. When the calendar is
        // detached, the launcher instead owns the results-list keys.
        let (calendar_active, calendar_detached) = {
            let state = self.state.borrow();
            let active = state.plugin_calendar.borrow().is_some();
            let detached = state.is_active_panel_detached();
            (active, detached)
        };
        if calendar_active && !calendar_detached {
            match keystroke.key.as_str() {
                "up" | "down" | "left" | "right" => {
                    let delta = match keystroke.key.as_str() {
                        "up" => -7,
                        "down" => 7,
                        "left" => -1,
                        _ => 1,
                    };
                    self.calendar_move(delta, cx);
                    cx.stop_propagation();
                    return;
                }
                "enter" => {
                    self.calendar_confirm(cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
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
                let count = self.input.char_count();
                self.input.set_cursor(count);
                cx.notify();
                cx.stop_propagation();
            }
            // Escape leaves the file-search page first (back to the home page,
            // box cleared), exactly like clicking the page's back control.
            // On the root page it hides the launcher directly at the key level
            // (the keybinding is a fallback): this is more robust than relying
            // on action dispatch when the window just went through a drag or
            // was re-activated.
            "escape" => {
                if self.page == LauncherPage::FileSearch {
                    self.leave_file_search(window, cx);
                } else {
                    self.dismiss_launcher(window, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Run the query only when no IME composition is in progress.
    ///
    /// While a composition is marked, the query holds the *in-line pre-edit*
    /// text — the pinyin being typed before a candidate is chosen — and it
    /// changes on every keystroke (`z` → `zh` → `zho` → `zhong`). Searching each
    /// of those produced a result list nobody asked for, discarded moments later,
    /// which is what made CJK input flicker. The pre-edit text stays visible in
    /// the box (it is underlined, and that is what the user is looking at), the
    /// drop-down keeps the rows from the last completed query, and the search
    /// runs once on the committed characters.
    ///
    /// Those kept rows are not actionable in the meantime: confirming one would
    /// launch something the pre-edit text does not name.
    fn search_unless_composing(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.input.marked.is_some() {
            self.results.set_confirmable(false, cx);
            cx.notify();
            return;
        }
        self.results.set_confirmable(true, cx);
        self.search(window, cx);
    }

    /// Run the current input through the engine and refresh the results list,
    /// mirroring the results for the confirm callback to resolve against, then
    /// resize the window to fit the new drop-down height. A query that is a
    /// complete arithmetic expression additionally gets a calculator row on
    /// top showing the computed value.
    ///
    /// The page decides what the query is matched against: the root page knows
    /// applications, plugins and the built-in commands, the file-search page
    /// only the file index. The two levels never mix rows, which is the whole
    /// point of the second level.
    pub(crate) fn search(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.page == LauncherPage::FileSearch {
            self.search_files(window, cx);
            return;
        }
        let query = self.input.query.clone();

        // The built-in "File Search" command: the home page always offers it
        // (that is how the second level is discovered), and a query that spells
        // its keyword puts it on top. The file index is not consulted on this
        // page at all — files belong to the page below — so an application list
        // can never be diluted by file hits.
        let offers_file_search =
            parse_file_search_command(&query).is_some() || !is_file_search(&query);
        // Whatever hits are in hand belong to the file-search page: drop them
        // and mark them superseded, so a late reply cannot repaint the root page
        // with rows nothing here asked for.
        self.clear_file_hits();
        self.file_search_at.set(None);

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
            .query_scored(&query, &|path| self.storage.borrow().frequency_str(path));

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
                .map(|(index, hit)| {
                    // `let` ends the temporary borrow before the miss path,
                    // so it can `borrow_mut` again.
                    let cached = state
                        .icon_cache
                        .borrow_mut()
                        .get(&hit.app.path)
                        .cloned()
                        .flatten();
                    if cached.is_some() {
                        return cached;
                    }
                    if index < MAX_RESULT_ROWS {
                        let icon = crate::app_icons::app_icon_image(&hit.app.path);
                        state
                            .icon_cache
                            .borrow_mut()
                            .insert(hit.app.path.clone(), icon.clone());
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
                    .filter(|hit| !state.icon_cache.borrow().contains_key(&hit.app.path))
                    .map(|hit| hit.app.path.clone())
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

        // Build the base rows: the built-in commands first, then applications.
        // Nothing here searches files, so the two kinds cannot be mixed.
        let mut rows: Vec<RankedRow> = Vec::new();
        if offers_file_search {
            rows.push(RankedRow {
                item: ResultItem::FileSearchCommand {
                    title: self.i18n.translate("file-search-title").to_owned(),
                    subtitle: self.i18n.translate("command").to_owned(),
                },
                icon: None,
                // A command the user spelled out leads every fuzzy name match —
                // even an application whose name *is* the query — so Enter does
                // what the keyword asked for.
                rank: RowRank {
                    tier: MatchTier::ExactName,
                    relevance: i64::MAX,
                    name_len: 0,
                    path: Vec::new(),
                },
            });
        }
        for item in items {
            rows.push(RankedRow {
                item,
                icon: None,
                // The other built-ins sit in the best tier too: a typed command
                // is never ambiguous with a name match.
                rank: RowRank::new(&query, "", i64::MAX, std::path::Path::new("")),
            });
        }
        let builtin_count = rows.len();
        rows.extend(apps.into_iter().zip(icons).map(|(hit, icon)| RankedRow {
            rank: RowRank::new(&query, &hit.app.name, i64::from(hit.score), &hit.app.path),
            item: ResultItem::App(hit.app),
            icon,
        }));
        self.base_rows = rows;
        self.builtin_count = builtin_count;
        self.ranked_query = query.clone();

        // Route the query to the matching plugins and invoke only those        // (capped to MAX_PLUGIN_ROWS) under a fresh generation. Views arrive
        // asynchronously; `render_merged` splices them in as they land and
        // stale generations are dropped by the poll task.
        let plugin_gen = {
            let state = self.state.borrow_mut();
            let gen = state.plugin_gen.get() + 1;
            state.plugin_gen.set(gen);
            gen
        };
        let hits = self
            .state
            .borrow()
            .plugin_host
            .borrow()
            .query(&query)
            .into_iter()
            .take(MAX_PLUGIN_ROWS)
            .collect::<Vec<_>>();
        *self.state.borrow().plugin_hits.borrow_mut() = hits.clone();
        *self.state.borrow().plugin_views.borrow_mut() = vec![None; hits.len()];
        // Reset the in-flight set: pending is scoped to the fresh generation.
        self.state.borrow().plugin_pending.borrow_mut().clear();
        let host = self.state.borrow().plugin_host.clone();
        for hit in &hits {
            let dispatched = host.borrow_mut().invoke(plugin_gen, hit);
            if dispatched.is_some() {
                self.state
                    .borrow()
                    .plugin_pending
                    .borrow_mut()
                    .insert((hit.plugin_id.clone(), hit.command.clone()));
            }
        }

        self.render_merged(
            current_file_generation(self),
            window,
            cx,
            Some(default_selection(
                LauncherPage::Root,
                &query,
                offers_file_search,
            )),
        );
    }

    /// Answer the file-search page: the query goes to the file index alone.
    ///
    /// Applications, plugins, the calculator and the link detector are all
    /// skipped, so a file row can never be mixed with a command row. An empty
    /// box is not a search — the index answers `""` with its first records in
    /// path order — so the page explains itself with a hint instead.
    fn search_files(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let query = self.input.query.clone();
        let mut items: Vec<ResultItem> = Vec::new();
        if !is_file_search(&query) {
            // Drop the hits in hand so rows from the previous query go away with
            // the text, and don't dispatch. Nothing is in flight for the emptied
            // box, so the next real query must not wait out the debounce of the
            // search the text just replaced.
            self.clear_file_hits();
            self.file_search_at.set(None);
            items.push(ResultItem::Hint {
                title: self.i18n.translate("files-type-to-search").to_owned(),
            });
        } else {
            // A burst of keystrokes would otherwise queue one search per
            // character, each answered with a full re-rank; only the first and
            // then at most one per `FILE_SEARCH_DEBOUNCE` are dispatched.
            let now = std::time::Instant::now();
            let due = self
                .file_search_at
                .get()
                .is_none_or(|last| now.duration_since(last) >= FILE_SEARCH_DEBOUNCE);
            if due {
                self.file_search_at.set(Some(now));
                {
                    let mut state = self.state.borrow_mut();
                    // The generation this request is about to be given. The
                    // reply carries it back, which is how the rows it produces
                    // are recognised as answering *this* query and no later one.
                    let generation = state.file_index.generation + 1;
                    state.file_search_generation = generation;
                }
                self.state.borrow_mut().file_index.search(
                    query.trim(),
                    steward_core_engine::file_index::KindFilter::Any,
                );
            }
            // A hint while the index is still being built, so an empty list is
            // explained rather than looking like "the search does not work".
            if let Some(hint) = self.index_hint() {
                items.push(ResultItem::Hint { title: hint });
            }
        }
        // The generation whose hits are about to be rendered. Every write to the
        // hit list records it, so this is the only thing that can prove the file
        // rows on screen were dispatched for the query in the box — `search` is
        // debounced, and a reply in hand may belong to a query the user has
        // already typed past.
        let file_generation = self.state.borrow().file_hits_generation;
        let builtin_rows = items
            .into_iter()
            .map(|item| RankedRow {
                item,
                icon: None,
                // A hint is not a match: it must never outrank a file row.
                rank: RowRank::new(&query, "", i64::MIN, std::path::Path::new("")),
            })
            .collect::<Vec<_>>();
        let builtin_count = builtin_rows.len();
        let mut rows = builtin_rows;
        rows.extend(self.file_rows(&query));
        self.base_rows = rows;
        self.builtin_count = builtin_count;
        self.ranked_query = query.clone();
        // The page's rows are complete as they stand (there is no plugin half),
        // and the producer records it so they stay confirmable while a debounced
        // reply is still in flight.
        self.results_query = self.input.query.clone();
        // Plugin routing is skipped on the page; the router state is reset so
        // stale plugin rows cannot linger.
        {
            let state = self.state.borrow();
            *state.plugin_hits.borrow_mut() = Vec::new();
            *state.plugin_views.borrow_mut() = Vec::new();
            state.plugin_pending.borrow_mut().clear();
        }
        self.render_merged(file_generation, window, cx, Some(0));
    }

    /// Drop the file-index hits in hand and mark the resulting empty set as
    /// the current one.
    ///
    /// Bumping the generation past the worker's is what makes a reply for a
    /// superseded query unrecognisable as "the answer to the query in the box":
    /// such a reply must not put file rows back under a query that no longer
    /// has any (an emptied box, or an `app:` scope).
    fn clear_file_hits(&self) {
        let mut state = self.state.borrow_mut();
        state.file_hits.clear();
        state.file_hits_generation = state.file_index.generation + 1;
    }

    /// Localized hint explaining an empty list: the file index is still being
    /// built, or unavailable. `None` when an index is ready, because then an
    /// empty list genuinely means "no match".
    fn index_hint(&self) -> Option<String> {
        let state = self.state.borrow();
        if state.file_index.is_ready() {
            None
        } else if state.file_index.is_building() {
            Some(self.i18n.translate("files-indexing").to_owned())
        } else {
            Some(self.i18n.translate("files-no-index").to_owned())
        }
    }

    /// The current file hits as ranked rows, each with its shell icon.
    ///
    /// An empty needle yields no rows whatever the hit list says: the page opens
    /// on a hint, files/folders appear only once something has been typed. This
    /// is the second half of the guarantee `search_files` makes when it clears
    /// the hits — a late reply cannot leak rows into an empty page.
    fn file_rows(&self, needle: &str) -> Vec<RankedRow> {
        if !is_file_search(needle) {
            return Vec::new();
        }
        let state = self.state.borrow();
        state
            .file_hits
            .iter()
            .take(crate::file_index::FILE_RESULT_LIMIT)
            .map(|hit| {
                // Files share the app icon cache: extraction is a shell call
                // keyed by path, so a file that is also a launchable app reuses
                // its entry.
                let icon = {
                    let cached = state
                        .icon_cache
                        .borrow_mut()
                        .get(&hit.path)
                        .cloned()
                        .flatten();
                    if cached.is_some() {
                        cached
                    } else {
                        let icon = crate::app_icons::app_icon_image(&hit.path);
                        state
                            .icon_cache
                            .borrow_mut()
                            .insert(hit.path.clone(), icon.clone());
                        icon
                    }
                };
                RankedRow {
                    icon,
                    rank: RowRank::new(needle, &hit.name, i64::from(hit.score), &hit.path),
                    item: ResultItem::File {
                        path: hit.path.clone(),
                        name: hit.name.clone(),
                        subtitle: file_subtitle(hit),
                        size: file_size(hit),
                    },
                }
            })
            .collect()
    }

    /// Merge the plugin rows with the base rows, push the result to the list view
    /// and resize the window. Used by `search` and by the poll task when a plugin
    /// view lands.
    /// ile_generation is the index worker generation whose hits are being
    /// rendered; it decides whether the file rows answer the query in the box
    /// (see the confirmation gate at the end). Callers that are not rendering a
    /// search pass the current generation, which is a no-op there.
    ///
    /// `default_selection` is the row the highlight moves to when this render
    /// changed the row list (see `ResultList::set_results_with_default`); a
    /// render that only splices in plugin views passes `None`, so a view landing
    /// under the user's arrow keys never yanks the highlight back to the top.
    pub(crate) fn render_merged(
        &mut self,
        file_generation: u64,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
        default_selection: Option<usize>,
    ) {
        // The displayed calendar survives only while the current query still
        // yields its view: a new search resets `plugin_views` to `None`, so
        // the grid falls back to the row-based list.
        let active_calendar = {
            let state = self.state.borrow();
            let current = state.plugin_calendar.borrow().clone();
            match current {
                Some(active) => {
                    let hits = state.plugin_hits.borrow();
                    let views = state.plugin_views.borrow();
                    let still_active = hits.iter().zip(views.iter()).any(|(hit, view)| {
                        hit.plugin_id == active.plugin_id
                            && view.as_ref().is_some_and(|view| {
                                parse_calendar_view(
                                    view,
                                    &hit.plugin_id,
                                    &hit.command,
                                    hit.detachable,
                                )
                                .is_some()
                            })
                    });
                    if still_active {
                        Some(active)
                    } else {
                        // The view for this command is gone from the current
                        // query: drop it so a later `calendar` starts fresh.
                        *state.plugin_calendar.borrow_mut() = None;
                        None
                    }
                }
                None => None,
            }
        };
        // Whether the (still-active) calendar has been popped out into its own
        // window: while it is detached the launcher shows the command row, not
        // the grid, and keeps the data so docking it back can restore the grid.
        let detached = active_calendar.as_ref().is_some_and(|active| {
            self.state
                .borrow()
                .plugin_window_open(&active.plugin_id, &active.command)
        });
        let show_calendar_grid = active_calendar.is_some() && !detached;
        let (plugin_rows, calendar_rows) = self
            .state
            .borrow()
            .plugin_rows_and_calendar_rows(&self.i18n.translate("command"));
        // Resolve the single detachable list panel (if any) for the launcher's
        // generic "pop out" control. Ambiguous (two or more) detachable list
        // views suppress the control rather than guessing.
        self.detachable_list_target = {
            let state = self.state.borrow();
            let hits = state.plugin_hits.borrow();
            let views = state.plugin_views.borrow();
            let mut target = None;
            for (hit, view) in hits.iter().zip(views.iter()) {
                if hit.detachable && view.as_ref().is_some_and(is_panel_view) {
                    if target.is_some() {
                        target = None;
                        break;
                    }
                    target = Some((hit.plugin_id.clone(), hit.command.clone()));
                }
            }
            target
        };
        // Inline Virtual UI Tree: the first `ui` view among the current hits
        // that is not popped out. It replaces the results list, like the
        // calendar does, at a fixed height.
        let active_ui = {
            let state_ref = self.state.borrow();
            let hits = state_ref.plugin_hits.borrow();
            let views = state_ref.plugin_views.borrow();
            let mut found = None;
            for (hit, view) in hits.iter().zip(views.iter()) {
                let Some(view) = view.as_ref() else {
                    continue;
                };
                if state_ref.plugin_window_open(&hit.plugin_id, &hit.command) {
                    continue;
                }
                if let Ok(node) = steward_ui_components::virtual_tree::validate_view(view) {
                    found = Some((hit.plugin_id.clone(), hit.command.clone(), node));
                    break;
                }
            }
            found
        };
        match active_ui {
            Some((plugin_id, command, node)) => {
                if self.ui_target.as_ref() == Some(&(plugin_id.clone(), command.clone())) {
                    if let Some(entity) = self.ui_view.clone() {
                        entity.update(cx, |view, cx| view.set_tree(node, cx));
                    }
                } else {
                    let sink =
                        launcher_event_sink(self.state.clone(), plugin_id.clone(), command.clone());
                    let entity = cx.new(|cx| {
                        steward_ui_components::virtual_tree::VirtualTreeView::new(node, sink, cx)
                    });
                    self.ui_view = Some(entity);
                    self.ui_target = Some((plugin_id, command));
                }
            }
            None => {
                self.ui_view = None;
                self.ui_target = None;
            }
        }
        let ui_inline = self.ui_view.is_some();
        self.state.borrow().plugin_ui_inline.set(ui_inline);
        if show_calendar_grid {
            // A calendar view replaces the results list: push the month grid's
            // data, localized labels and selection into the calendar widget.
            let active = active_calendar
                .as_ref()
                .expect("show_calendar_grid implies Some");
            let language = self.i18n.language();
            self.calendar_selected = active.data.selected.clone();
            self.calendar.set_data(
                active.data.clone(),
                calendar_month_label(&language, active.data.year, active.data.month),
                calendar_weekday_labels(&language, active.data.start_of_week),
                active.data.selected.clone(),
                cx,
            );
            // Only a manifest-detachable command shows the detach control, and
            // inline (not popped out) it always renders in the unpinned state.
            self.calendar.set_detachable(active.detachable, cx);
            self.calendar.set_pinned(false, cx);
        }
        let plugin_count = plugin_rows.len() + calendar_rows.len();
        let builtin_count = self.builtin_count;
        // A transient "running" row while a plugin command is still in flight
        // (lazy-loading a cold plugin, or draining an async `command()`'s job
        // queue). Cleared the moment its `CommandResult` lands, so it never
        // lingers on an error or a completed view.
        let pending_command = {
            let state = self.state.borrow();
            let pending = state
                .plugin_pending
                .borrow()
                .iter()
                .next()
                .map(|(_, command)| command.clone());
            pending
        };
        // Icons for the plugin / calendar rows, resolved by plugin id from the
        // manifest cache, so they look like app rows.
        let plugin_icons = {
            let state = self.state.borrow();
            plugin_rows
                .iter()
                .chain(calendar_rows.iter())
                .map(|row| match row {
                    ResultItem::Plugin { plugin_id, .. }
                    | ResultItem::Calendar { plugin_id, .. }
                    | ResultItem::Command { plugin_id, .. } => state
                        .plugin_icons
                        .borrow()
                        .get(plugin_id)
                        .cloned()
                        .flatten(),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // Merge and rank. Commands the user typed (calculator, link, a plugin
        // keyword) occupy the best tier because they are unambiguous; everything
        // else — applications, plugin list items and indexed files — is ordered
        // purely by how well it matches, so an exactly-named file can lead the
        // list even when applications also contain the query's letters.
        let needle = self.ranked_query.clone();
        let mut rows = Vec::with_capacity(self.base_rows.len() + plugin_count);
        for (offset, row) in self.base_rows.iter().enumerate() {
            if offset == builtin_count {
                if let Some(command) = pending_command.clone() {
                    rows.push(RankedRow {
                        item: ResultItem::Loading { command },
                        icon: None,
                        rank: RowRank {
                            tier: MatchTier::NameSubstring,
                            relevance: i64::MAX,
                            name_len: 0,
                            path: Vec::new(),
                        },
                    });
                }
            }
            rows.push(RankedRow {
                item: row.item.clone(),
                icon: row.icon.clone(),
                rank: row.rank.clone(),
            });
        }
        if builtin_count >= self.base_rows.len() {
            if let Some(command) = pending_command {
                rows.push(RankedRow {
                    item: ResultItem::Loading { command },
                    icon: None,
                    rank: RowRank {
                        tier: MatchTier::NameSubstring,
                        relevance: i64::MAX,
                        name_len: 0,
                        path: Vec::new(),
                    },
                });
            }
        }
        for (row, icon) in plugin_rows
            .into_iter()
            .chain(calendar_rows)
            .zip(plugin_icons)
        {
            let (title, command) = match &row {
                ResultItem::Command { title, command, .. }
                | ResultItem::Calendar { title, command, .. } => (title.clone(), command.clone()),
                _ => (String::new(), String::new()),
            };
            rows.push(RankedRow {
                item: row,
                icon,
                rank: plugin_row_rank(&needle, &title, &command),
            });
        }
        rank_rows(&mut rows);

        let mut items = Vec::with_capacity(rows.len());
        let mut icons = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(row.item);
            icons.push(row.icon);
        }

        *self.last_results.borrow_mut() = items.clone();
        self.results
            .set_results_with_default(items, icons, default_selection, cx);

        // These rows are actionable only when they answer what is in the box.
        //
        // For a search render the evidence is the file generation: `search` is
        // debounced, so a reply in hand may belong to a query the user has typed
        // past, and only a generation match proves the hits on screen were
        // dispatched for the input as it stands. A render that is not answering a
        // search (a plugin view landing) passes the current generation, which is
        // honest — it renders whatever is in hand — and `results_query`, which the
        // producer sets, is what vouches for those rows.
        //
        // An IME composition suspends confirmation entirely: the pre-edit text in
        // the box is not what any of these rows answer.
        let file_fresh = file_generation == self.state.borrow().file_hits_generation;
        let active_query = self.input.query.clone();
        let query_fresh = self.results_query == active_query;
        let composing = self.input.marked.is_some();
        let fresh = (file_fresh || query_fresh) && !composing;
        self.results.set_confirmable(fresh, cx);

        let count = self.results.visible_count(cx);
        let height = if show_calendar_grid {
            let active = active_calendar
                .as_ref()
                .expect("show_calendar_grid implies Some");
            calendar_height(&active.data)
        } else if ui_inline {
            ui_inline_height()
        } else {
            launcher_height(count)
        };
        let mut state = self.state.borrow_mut();
        state.result_count.set(count);
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
            window.resize(size(px(state.width()), px(height)));
        }
        cx.notify();
    }

    /// Merge a freshly arrived plugin view into the current results. Called by
    /// the foreground poll task after the view was stored in the shared state;
    /// never re-invokes plugins.
    pub(crate) fn apply_plugin_views(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.render_merged(current_file_generation(self), window, cx, None);
    }

    /// Move the calendar selection by `delta` days (clamped to the displayed
    /// month), updating both the widget highlight and the confirm target.
    fn calendar_move(&mut self, delta: i32, cx: &mut gpui::Context<Self>) {
        let Some((year, month, day)) = parse_iso_date(&self.calendar_selected) else {
            return;
        };
        let days = days_in_month(year, month);
        let new_day = (day as i32 + delta).clamp(1, days as i32) as u32;
        let iso = iso_date(year, month, new_day);
        self.calendar_selected = iso.clone();
        self.calendar.set_selected(&iso, cx);
        cx.notify();
    }

    /// Confirm the selected calendar day: dispatch `item.invoke` to the plugin
    /// that produced the view and keep the launcher open.
    fn calendar_confirm(&mut self, _cx: &mut gpui::Context<Self>) {
        let Some(active) = self.state.borrow().plugin_calendar.borrow().clone() else {
            return;
        };
        let selected = self.calendar_selected.clone();
        if self
            .state
            .borrow()
            .plugin_host
            .borrow_mut()
            .invoke_item(&active.plugin_id, &active.command, &selected)
            .is_none()
        {
            eprintln!(
                "[steward] plugin {} not ready for item.invoke",
                active.plugin_id
            );
        }
    }

    /// Drill into the file-search page.
    ///
    /// The text after the keyword travels with the drill-down (`fs report` opens
    /// the page already searching `report`), so the keyword is a real entry
    /// point and not just a switch. Called from the confirm handler, i.e. after
    /// Enter or a click on the command row.
    pub(crate) fn enter_file_search(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        // The switch is deferred past the key event (see the confirm handler),
        // so the box may have moved on in the meantime: enter only when the row
        // still describes what is typed. The home page's fixed entry row is the
        // empty-box case.
        let remainder = if self.input.query.trim().is_empty() {
            String::new()
        } else if let Some(remainder) = parse_file_search_command(&self.input.query) {
            remainder.to_owned()
        } else {
            return;
        };
        self.show_page(LauncherPage::FileSearch, &remainder, window, cx);
    }

    /// Leave the file-search page: back to the root level with the box cleared,
    /// which is the launcher's home page (the most-used applications).
    fn leave_file_search(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.show_page(LauncherPage::Root, "", window, cx);
    }

    /// Switch level, replacing the query with `query` and re-running the search.
    fn show_page(
        &mut self,
        page: LauncherPage,
        query: &str,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.page = page;
        self.input.query = query.to_owned();
        self.input.set_cursor(self.input.char_count());
        self.input.marked = None;
        self.input.selection = None;
        self.mouse_selecting = false;
        self.mouse_anchor = 0;
        self.search(window, cx);
    }

    /// Reset the launcher to its idle (bar-only) state and hide it. Called
    /// after the delegate's confirm callback has launched the selected app.
    fn after_confirm(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        // Confirming ends the visit whichever level it was on: the next summon
        // opens on the root page again.
        self.page = LauncherPage::Root;
        let input = &mut self.input;
        input.query.clear();
        input.cursor = 0;
        input.marked = None;
        input.selection = None;
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
        let anchor = self.mouse_anchor;
        self.input.select_anchor_to(anchor, index);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_view_items_unwraps_response_envelope() {
        let wrapped = serde_json::json!({
            "view": {
                "type": "list",
                "items": [{ "id": "a", "title": "Alpha", "subtitle": "first" }]
            }
        });
        let items = plugin_view_items(&wrapped).expect("wrapped list view");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a");

        // A bare view (no envelope) is tolerated too.
        let bare = serde_json::json!({ "type": "list", "items": [] });
        assert_eq!(plugin_view_items(&bare).map(|items| items.len()), Some(0));

        // Non-list views and missing envelopes yield no rows.
        let not_list = serde_json::json!({ "view": { "type": "detail" } });
        assert!(plugin_view_items(&not_list).is_none());
        let missing = serde_json::json!({ "view": {} });
        assert!(plugin_view_items(&missing).is_none());
    }

    #[test]
    fn parse_calendar_view_handles_wrapped_and_bare_views() {
        let wrapped = serde_json::json!({
            "view": {
                "type": "calendar",
                "year": 2026,
                "month": 8,
                "today": "2026-08-27",
                "startOfWeek": 1,
                "selected": "2026-08-27"
            }
        });
        let calendar = parse_calendar_view(&wrapped, "com.example.calendar", "calendar", true)
            .expect("calendar view");
        assert_eq!(calendar.plugin_id, "com.example.calendar");
        assert_eq!(calendar.command, "calendar");
        assert!(calendar.detachable);
        assert_eq!(calendar.data.year, 2026);
        assert_eq!(calendar.data.month, 8);
        assert_eq!(calendar.data.today, "2026-08-27");
        assert_eq!(calendar.data.start_of_week, 1);

        // Bare view (no envelope) and defaulted fields are tolerated.
        let bare = serde_json::json!({
            "type": "calendar",
            "year": 2026,
            "month": 13,
            "today": "2026-01-01"
        });
        let calendar =
            parse_calendar_view(&bare, "com.example.calendar", "calendar", false).unwrap();
        assert!(!calendar.detachable);
        assert_eq!(calendar.data.month, 12, "month clamps to 1..=12");
        assert_eq!(
            calendar.data.selected, "2026-01-01",
            "selected defaults to today"
        );
        assert_eq!(
            calendar.data.start_of_week, 1,
            "startOfWeek defaults to Monday"
        );

        // Non-calendar and malformed views yield no calendar.
        assert!(parse_calendar_view(
            &serde_json::json!({ "view": { "type": "list" } }),
            "p",
            "list",
            false
        )
        .is_none());
        assert!(parse_calendar_view(
            &serde_json::json!({ "view": { "type": "calendar" } }),
            "p",
            "calendar",
            false
        )
        .is_none());
    }

    #[test]
    fn iso_date_parsing_and_navigation_helpers() {
        assert_eq!(parse_iso_date("2026-08-27"), Some((2026, 8, 27)));
        assert_eq!(
            parse_iso_date("2026-8-27"),
            None,
            "month must be zero-padded"
        );
        assert_eq!(parse_iso_date("garbage"), None);
        assert_eq!(parse_iso_date("2026-08-27-extra"), None);

        assert_eq!(calendar_month_label("zh", 2026, 8), "2026 年 8 月");
        assert_eq!(calendar_month_label("en", 2026, 8), "August 2026");

        let monday_first = calendar_weekday_labels("zh", 1);
        assert_eq!(monday_first, ["一", "二", "三", "四", "五", "六", "日"]);
        let sunday_first = calendar_weekday_labels("en", 0);
        assert_eq!(sunday_first, ["S", "M", "T", "W", "T", "F", "S"]);
    }

    #[test]
    fn the_file_search_keyword_hands_the_rest_of_the_query_to_the_page() {
        // The keyword is the page's entry point: everything after it becomes
        // the file query, so `fs report` drills in already searching.
        assert_eq!(parse_file_search_command("fs"), Some(""));
        assert_eq!(parse_file_search_command("fs report"), Some("report"));
        assert_eq!(
            parse_file_search_command("fs  annual report"),
            Some("annual report")
        );
        // Case and leading whitespace do not matter.
        assert_eq!(parse_file_search_command("FS report"), Some("report"));
        assert_eq!(parse_file_search_command("  fs report"), Some("report"));
        // The full names are keywords too, in both scripts.
        assert_eq!(parse_file_search_command("file search x"), Some("x"));
        assert_eq!(parse_file_search_command("文件搜索 报告"), Some("报告"));
        // A keyword has to be a whole token, and only a leading one counts.
        assert_eq!(parse_file_search_command("fsx"), None);
        assert_eq!(parse_file_search_command("files"), None);
        assert_eq!(parse_file_search_command("report fs"), None);
        assert_eq!(parse_file_search_command("report"), None);
        // An empty query is the home page, not a command.
        assert_eq!(parse_file_search_command(""), None);
        assert_eq!(parse_file_search_command("   "), None);
    }

    #[test]
    fn the_highlight_lands_on_the_row_the_query_produced() {
        // A keyword hands the highlight to the command row, so Enter enters the
        // page without a Down press.
        assert_eq!(
            default_selection(LauncherPage::Root, "fs", true),
            0,
            "a spelled-out command is the row the user asked for"
        );
        // The home page offers the same command on top for discoverability but
        // still opens on the most-used application: summon + Enter keeps
        // launching it.
        assert_eq!(default_selection(LauncherPage::Root, "", true), 1);
        assert_eq!(
            default_selection(LauncherPage::Root, "chrome", false),
            0,
            "without the command row the top match is the first row"
        );
        // The page itself always opens on its first row (hint or first hit).
        assert_eq!(default_selection(LauncherPage::FileSearch, "", false), 0);
        assert_eq!(
            default_selection(LauncherPage::FileSearch, "report", false),
            0
        );
    }

    #[test]
    fn a_file_name_match_outranks_a_weaker_app_match() {
        // The launcher's whole reason for ranking across kinds: a file whose name
        // *is* the query must lead, even though applications are matched fuzzily
        // and used to be listed first unconditionally.
        let app = RowRank::new(
            "report",
            "Repository Tools",
            900,
            std::path::Path::new("C:/apps/repo-tools.exe"),
        );
        let file = RowRank::new(
            "report",
            "report.pdf",
            0,
            std::path::Path::new("D:/report.pdf"),
        );
        assert_eq!(file.tier, steward_core_engine::MatchTier::ExactName);
        assert_eq!(app.tier, steward_core_engine::MatchTier::Weak);
        // Tier dominates the fuzzy score.
        assert_eq!(file.compare(&app), std::cmp::Ordering::Less);

        // Within a tier the relevance decides, so a stronger fuzzy hit still wins.
        let strong = RowRank::new("report", "zzz", 900, std::path::Path::new("C:/a.exe"));
        let weak = RowRank::new("report", "zzz", 100, std::path::Path::new("C:/b.exe"));
        assert_eq!(strong.compare(&weak), std::cmp::Ordering::Less);
    }

    #[test]
    fn the_home_page_is_not_a_file_search() {
        // An empty (or whitespace-only) query is the launcher's home page: it
        // must not reach the file index or produce file rows, or the index's
        // "list everything" answer would take the page over.
        assert!(!is_file_search(""));
        assert!(!is_file_search("   "));
        assert!(!is_file_search("\t"));
        // Any real term is a search, in either script.
        assert!(is_file_search("report"));
        assert!(is_file_search("报告"));
    }
}
