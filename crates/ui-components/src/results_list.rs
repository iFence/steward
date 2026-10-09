//! A simple, non-virtualized results list for the launcher.
//!
//! The launcher caps its drop-down at `MAX_RESULT_ROWS` (8) rows, so M1 does
//! not need a virtualized list. The initial M1 implementation reused
//! `gpui-component`'s `List`/virtual list; on Windows it rendered rows at the
//! wrong positions (faint or missing row text) and painted a stray white quad
//! in the bottom-right corner of the drop-down. A plain stacked `div` list
//! gives full control over layout, selection, hover and colors, and removes
//! that rendering path entirely.
//!
//! The search box is owned by the `app` crate: each keystroke runs a query
//! there and pushes the rows plus their (optional) icons via
//! [`ResultList::set_results`]. Confirmation (Enter / click) is surfaced back
//! through the `on_confirm` callback so the app can launch the application and
//! bump its usage frequency.

use std::{rc::Rc, sync::Arc};

use gpui::{
    div, img, prelude::FluentBuilder, px, rgb, App, AppContext, Context, ElementId, Entity, Image,
    ImageSource, InteractiveElement, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement, Styled as _,
};

use steward_core_engine::AppEntry;

/// Delegate callback fired on Enter / click with the confirmed row index.
/// Returns whether the launcher should hide after confirming (`true`). Plugin
/// rows return `false` so the bar stays open after an `item.invoke`.
pub type ConfirmCallback = Rc<dyn Fn(usize, &mut App) -> bool>;

/// A row in the launcher drop-down. Either a launchable application or a
/// one-off action such as a calculator result — an action row shows its own
/// title and subtitle instead of an icon plus the application label, and its
/// confirmation runs the app-side `on_confirm` (which copies the computed
/// value to the clipboard rather than launching anything). A link row is the
/// "open in browser" command: it shows the URL and, on confirm, opens it in
/// the default browser. A plugin row is one list item rendered by a plugin
/// command; confirming sends `item.invoke` and keeps the launcher open.
///
/// `PartialEq` is what lets [`ResultListState::set_results`] recognise a
/// re-pushed, unchanged list and leave the selection alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultItem {
    App(AppEntry),
    Action {
        title: String,
        subtitle: String,
    },
    Link {
        url: String,
        label: String,
        command_label: String,
    },
    Plugin {
        plugin_id: String,
        /// The command that produced this plugin row; the host uses it to
        /// route an `item.invoke`-returned view back to the right command.
        command: String,
        item_id: String,
        title: String,
        subtitle: String,
    },
    /// A plugin command row whose confirmed view is a calendar: confirming
    /// reveals the month grid (the app owns the view) and keeps the launcher
    /// open. Rendered like a plugin row, with the plugin's icon when present.
    Calendar {
        plugin_id: String,
        command: String,
        title: String,
        subtitle: String,
    },
    /// A file or folder from the full-disk index. The row carries three
    /// columns — name, size, containing folder — so a long path can only
    /// shorten itself: it can never push the size into the middle of the path.
    /// There is no "File" / "Folder" tag: the row's icon already says which it
    /// is. Confirming opens the entry with the OS default handler, or reveals
    /// the folder when the match is a directory.
    File {
        path: std::path::PathBuf,
        name: String,
        /// The containing folder, alone.
        subtitle: String,
        /// Formatted byte count ("120.6 KB"), or empty for a folder.
        size: String,
    },
    /// A plugin command's entry row: confirming it opens the plugin's view in
    /// its own independent application window (instead of flattening the view
    /// into the launcher drop-down). Applies to every plugin command, so each
    /// plugin behaves like a launched app.
    Command {
        plugin_id: String,
        command: String,
        title: String,
        subtitle: String,
    },
    /// The launcher's built-in "File Search" command: confirming it does not
    /// launch anything, it drills into the launcher's own file-search page
    /// (the second level, where the query is answered by the file index alone).
    FileSearchCommand {
        title: String,
        subtitle: String,
    },
    /// A non-actionable line of text, e.g. the hint shown on the file-search
    /// page while its box is empty. Confirming it does nothing at all: unlike
    /// [`ResultItem::Action`] it must not copy its own text to the clipboard,
    /// and unlike [`ResultItem::Loading`] it is not waiting for anything.
    Hint {
        title: String,
    },
    /// A transient placeholder shown while a plugin command is still running
    /// (lazy-loading a cold plugin, or draining an async `command()`'s
    /// micro-tasks). Not confirmable: it only signals that a row is coming.
    Loading {
        command: String,
    },
}

/// Design (96-DPI) geometry of a result row, in logical pixels. GPUI scales
/// these with the display DPI like any other app UI.
const DESIGN_ROW_HEIGHT: f32 = 42.0;
const DESIGN_ICON_SIZE: f32 = 24.0;
/// Rows visible at once in the drop-down. Must match the app's
/// `MAX_RESULT_ROWS`; the list renders exactly this many rows so the pinned
/// GPUI Windows revision never has to clip overflowing children (its scroll
/// container paints them unclipped, spilling below the drop-down).
pub const VISIBLE_ROWS: usize = 8;
/// Width of the secondary text column (a file row's folder, an app row's kind
/// label, a plugin row's subtitle). Fixed, and applied on every row, so the
/// column's edge lands in one place instead of drifting with the length of each
/// string; the text inside it is right-aligned, so short labels end at that
/// common edge rather than floating in the middle of the column.
const DETAIL_COLUMN_WIDTH: f32 = 280.0;
/// Width reserved for the size column. Fixed — and reserved on every row, file
/// or not — so the trailing columns line up down the whole drop-down.
const SIZE_COLUMN_WIDTH: f32 = 64.0;
/// The state backing the results list. Kept as its own entity so updates
/// (`set_results`, selection moves) can happen without a window.
pub struct ResultListState {
    items: Vec<ResultItem>,
    icons: Vec<Option<Arc<Image>>>,
    type_label: String,
    max_height: f32,
    selected: Option<usize>,
    on_confirm: Option<ConfirmCallback>,
    /// Opacity of the white selection wash, adapted by the app to the current
    /// scrim (raised over bright backdrops, where a fixed 0.10 wash reads too
    /// faint against the lightened bar).
    selected_wash: f32,
    /// Whether the displayed rows may be confirmed. The launcher turns this off
    /// while a search is in flight, so rows kept on screen for a smooth repaint
    /// cannot be confirmed by mistake. See [`ResultList::set_confirmable`].
    confirmable: bool,
}

impl Render for ResultListState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let type_label = self.type_label.clone();
        let range = self.visible_range();
        let selected = self.selected;
        let selected_wash = self.selected_wash;
        let rows = self.items[range.clone()]
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let index = range.start + offset;
                render_row(
                    item,
                    self.icons.get(index).cloned().flatten(),
                    &type_label,
                    selected == Some(index),
                    selected_wash,
                    index,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        // Exactly `VISIBLE_ROWS` rows are rendered; nothing overflows, so the
        // container needs no scroll/clip machinery (which is broken in the
        // pinned GPUI revision on Windows).
        div()
            .id(ElementId::from("results-rows"))
            .flex()
            .flex_col()
            .w_full()
            .children(rows)
    }
}

/// Whether two icon lists are the same. `Arc` identity is enough: icons are
/// cached and shared, so a re-pushed list holds the same pointers.
fn icons_equal(left: &[Option<Arc<Image>>], right: &[Option<Arc<Image>>]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        })
}

/// Which row to highlight after the row list changed.
///
/// `default_selection` is the row the producer wants the highlight on: the
/// launcher passes the command row a keystroke just produced, or the first
/// application on its home page. Without a default the old index is kept while
/// it still points at a row (and a shorter list falls back to the first row),
/// so re-pushing an identical list or filling icons in never moves the
/// highlight the user is arrowing through.
fn next_selection(
    previous: Option<usize>,
    len: usize,
    default_selection: Option<usize>,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match default_selection {
        Some(index) => Some(index.min(len - 1)),
        None => previous.filter(|&index| index < len).or(Some(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_core_engine::AppEntry;

    fn row(name: &str) -> ResultItem {
        ResultItem::App(AppEntry {
            name: name.to_owned(),
            path: std::path::PathBuf::from(format!("C:/{name}.exe")),
        })
    }

    #[test]
    fn identical_rows_compare_equal() {
        assert_eq!(row("a"), row("a"));
        assert_ne!(row("a"), row("b"));
        // Order matters: a reordered list is a different list.
        assert_ne!(vec![row("a"), row("b")], vec![row("b"), row("a")]);
    }

    #[test]
    fn icon_lists_compare_by_pointer() {
        assert!(icons_equal(&[], &[]));
        assert!(icons_equal(&[None], &[None]));
        assert!(!icons_equal(&[None], &[]));
        // A `Some` slot is only "the same" when it is the same allocation, which
        // is what a cached icon re-pushed by the app looks like.
        let shared = Arc::new(Image::from_bytes(gpui::ImageFormat::Png, Vec::new()));
        assert!(icons_equal(
            &[Some(shared.clone())],
            &[Some(shared.clone())]
        ));
        assert!(!icons_equal(
            &[Some(shared.clone())],
            &[Some(Arc::new(Image::from_bytes(
                gpui::ImageFormat::Png,
                Vec::new()
            )))]
        ));
        assert!(!icons_equal(&[Some(shared)], &[None]));
    }

    #[test]
    fn a_keystroke_hands_the_highlight_to_the_row_it_produced() {
        // The launcher's "fs" flow: the home page has the file-search command
        // pinned on top but opens on the first application, and typing the
        // keyword moves the highlight onto that command row.
        assert_eq!(next_selection(None, 3, Some(1)), Some(1));
        assert_eq!(next_selection(Some(1), 3, Some(0)), Some(0));
        // A default past the end of a shorter list clamps to the last row.
        assert_eq!(next_selection(Some(0), 2, Some(7)), Some(1));
        // An empty list has nothing to select, whatever the producer asks for.
        assert_eq!(next_selection(Some(2), 0, Some(0)), None);
        assert_eq!(next_selection(None, 0, None), None);
    }

    #[test]
    fn without_a_default_the_old_row_is_kept_while_it_exists() {
        // Re-pushing identical rows or filling the icon cache must not move the
        // highlight the user is arrowing through.
        assert_eq!(next_selection(Some(2), 5, None), Some(2));
        // A row that no longer exists falls back to the top match.
        assert_eq!(next_selection(Some(4), 3, None), Some(0));
        assert_eq!(next_selection(None, 3, None), Some(0));
    }
}

impl ResultListState {
    /// The slice of items to render: a `VISIBLE_ROWS`-tall window anchored so
    /// the selected row is always visible (at the bottom once the list is long
    /// enough to scroll).
    fn visible_range(&self) -> std::ops::Range<usize> {
        let viewport_rows = (self.max_height / DESIGN_ROW_HEIGHT).max(1.0) as usize;
        let len = self.items.len();
        let top = self
            .selected
            .unwrap_or(0)
            .min(len.saturating_sub(1))
            .saturating_sub(viewport_rows.saturating_sub(1))
            .min(len);
        top..(top + viewport_rows).min(len)
    }
}

/// A result row: fixed height (matches the app's row metric). App rows show an
/// icon (when available), name and the localized application label on the
/// right; action rows (calculator results) show the computed value on the left
/// and the original expression on the right. Selected rows get Tinycast's
/// neutral white wash (opacity adapted by the app); hovered rows get the
/// fainter white 0.05 surface tint.
fn render_row(
    item: &ResultItem,
    icon: Option<Arc<Image>>,
    type_label: &str,
    selected: bool,
    selected_wash: f32,
    index: usize,
    cx: &mut Context<ResultListState>,
) -> impl IntoElement {
    let id = match item {
        ResultItem::App(app) => ElementId::from(app.path.to_string_lossy().into_owned()),
        ResultItem::Action { .. } => ElementId::from(format!("result-action-{index}")),
        ResultItem::Link { .. } => ElementId::from(format!("result-link-{index}")),
        ResultItem::Plugin { .. } => ElementId::from(format!("result-plugin-{index}")),
        ResultItem::Calendar { .. } => ElementId::from(format!("result-calendar-{index}")),
        ResultItem::File { path, .. } => ElementId::from(path.to_string_lossy().into_owned()),
        ResultItem::Command { .. } => ElementId::from(format!("result-command-{index}")),
        ResultItem::FileSearchCommand { .. } => {
            ElementId::from(format!("result-file-search-{index}"))
        }
        ResultItem::Hint { .. } => ElementId::from(format!("result-hint-{index}")),
        ResultItem::Loading { .. } => ElementId::from(format!("result-loading-{index}")),
    };
    let row = div()
        .id(id)
        .h(px(DESIGN_ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .cursor_pointer()
        // No own background: the window root paints the single translucent
        // scrim (palette::BACKGROUND at palette::SCRIM_ALPHA) across the whole
        // launcher, so rows stay transparent and the frosted-glass backdrop
        // shows uniformly under the drop-down too.
        .when(selected, |this| {
            this.bg(rgb(crate::palette::SELECTION).opacity(selected_wash))
        })
        .when(!selected, |this| {
            this.hover(|style| style.bg(rgb(crate::palette::HOVER).opacity(0.05)))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            // A click both selects the row and confirms it, so a
            // subsequent Enter (via the owner's list) always has a
            // selection to act on.
            this.selected = Some(index);
            if let Some(cb) = this.on_confirm.clone() {
                let _ = cb(index, cx);
            }
            cx.notify();
        }));

    // Every arm below pairs the row's main text with its trailing columns: the
    // name, and optionally the size (file rows) and the secondary text. Splitting
    // the text out here is what lets the trailing columns stay put without
    // repeating the layout in every arm. `None` means the row has that column
    // and leaves it out entirely.
    let (row, name, size, detail, icon) = match item {
        ResultItem::App(app) => (
            row,
            app.name.to_owned(),
            None,
            Some(type_label.to_owned()),
            icon,
        ),
        ResultItem::Action { title, subtitle } => {
            (row, title.to_owned(), None, Some(subtitle.to_owned()), None)
        }
        // A link row mirrors the action-row layout: the command label ("Open
        // in Browser") on the left, and the "Command" type tag on the right —
        // the URL itself is only carried for the confirm handler to open.
        ResultItem::Link {
            label,
            command_label,
            ..
        } => (
            row,
            label.to_owned(),
            None,
            Some(command_label.to_owned()),
            None,
        ),
        // A plugin row mirrors the app-row layout: the manifest's SVG icon
        // (when declared), item title on the left, its subtitle on the right.
        // Confirming dispatches `item.invoke` to the plugin and keeps the
        // launcher open.
        ResultItem::Plugin {
            title, subtitle, ..
        }
        | ResultItem::Calendar {
            title, subtitle, ..
        }
        | ResultItem::Command {
            title, subtitle, ..
        } => (row, title.to_owned(), None, Some(subtitle.to_owned()), icon),
        // The built-in file-search command mirrors the link row: the command
        // name on the left, its type tag on the right, no icon.
        ResultItem::FileSearchCommand { title, subtitle } => {
            (row, title.to_owned(), None, Some(subtitle.to_owned()), None)
        }
        // A hint is a label, not a row to act on: a plain title with no
        // trailing column and no icon (confirming it is a no-op).
        ResultItem::Hint { title } => (row, title.to_owned(), None, None, None),
        // A file row carries three columns: the name, the size and the
        // containing folder. Each is its own cell, so a long path can only
        // shorten itself — it can never push the size into the middle of the
        // path. There is no "File" / "Folder" tag: the icon already says which
        // it is, and the column cost more than it told.
        ResultItem::File {
            name,
            subtitle,
            size,
            ..
        } => (
            row,
            name.clone(),
            Some(size.clone()),
            Some(subtitle.clone()),
            icon,
        ),
        // A loading placeholder: muted title on the left, a pulse on the
        // right. It is intentionally not confirmable (see the app's confirm
        // handler) and disappears as soon as the command's view lands.
        ResultItem::Loading { command } => {
            (row, command.to_owned(), None, Some("…".to_owned()), None)
        }
    };

    let name = div()
        .flex_1()
        .truncate()
        .text_color(rgb(crate::palette::FOREGROUND))
        .text_sm()
        .child(name);

    // The row reads left to right: the icon, the name, the size, then the
    // secondary text.
    //
    // Both text columns truncate to the right and are laid out left to right, so
    // the name gives way to the size and the size never moves: the name is the
    // flexible column and the two trailing ones are reserved on every row that
    // has them, which is what keeps the columns on one axis down the drop-down.
    // A row with nothing to put in them drops them entirely instead of
    // reserving two empty columns against its own text.
    //
    // Inside their reserved width the trailing columns align their text to the
    // right, so a short label ("Application" / "Command") sits at the row's
    // right edge instead of floating in the middle of a 280px column. The
    // widths are unchanged, so the columns still line up down the list.
    let mut row = row.when_some(icon, |this, icon| {
        this.child(
            img(ImageSource::Image(icon))
                .w(px(DESIGN_ICON_SIZE))
                .h(px(DESIGN_ICON_SIZE)),
        )
    });
    row = row.child(name);
    if let Some(size) = size {
        row = row.child(
            div()
                .w(px(SIZE_COLUMN_WIDTH))
                .flex_shrink_0()
                .truncate()
                .text_color(rgb(crate::palette::MUTED_FOREGROUND))
                .text_size(px(11.0))
                .text_right()
                .child(size),
        );
    }
    if let Some(detail) = detail {
        row = row.child(
            div()
                .w(px(DETAIL_COLUMN_WIDTH))
                .flex_shrink_0()
                .truncate()
                .text_color(rgb(crate::palette::MUTED_FOREGROUND))
                .text_size(px(11.0))
                .text_right()
                .child(detail),
        );
    }
    row
}

/// Builds the `ResultList` with an optional confirm callback.
pub struct ResultListDelegate {
    on_confirm: Option<ConfirmCallback>,
    type_label: String,
}

impl ResultListDelegate {
    pub fn new() -> Self {
        Self {
            on_confirm: None,
            type_label: "Application".into(),
        }
    }

    /// Label shown on the right of every application row (the localized word
    /// for "Application"), replacing the raw executable path.
    pub fn type_label(mut self, label: impl Into<String>) -> Self {
        self.type_label = label.into();
        self
    }

    /// Register a callback fired with the confirmed row index (Enter / click).
    /// The `&mut App` lets the app write to the clipboard for action rows; the
    /// returned bool says whether the launcher should hide afterwards.
    pub fn on_confirm(mut self, cb: impl Fn(usize, &mut App) -> bool + 'static) -> Self {
        self.on_confirm = Some(Rc::new(cb));
        self
    }
}

impl Default for ResultListDelegate {
    fn default() -> Self {
        Self::new()
    }
}

/// The launcher results list: a plain stacked list of rows. Rows are pushed in
/// from the app; the drop-down height is derived from
/// [`ResultList::visible_count`] by the enclosing window.
#[derive(Clone)]
pub struct ResultList {
    state: Entity<ResultListState>,
}

impl ResultList {
    /// Create the list inside a window context (from the app's root entity).
    pub fn new<C>(
        delegate: ResultListDelegate,
        _window: &mut gpui::Window,
        cx: &mut Context<C>,
    ) -> Self {
        let state = cx.new(|_| ResultListState {
            items: Vec::new(),
            icons: Vec::new(),
            type_label: delegate.type_label,
            max_height: 0.0,
            selected: None,
            on_confirm: delegate.on_confirm,
            selected_wash: crate::palette::SELECTION_WASH,
            confirmable: true,
        });
        Self { state }
    }

    /// Replace the displayed rows (and their icons, aligned with `items`) and
    /// re-render. Called by the app after every search; no window access is
    /// required (works from a plain entity context).
    ///
    /// Rows that are equal to what is already displayed, and icons that are
    /// unchanged, are left alone — and the selection with them. A caller that
    /// re-pushes the same list (an index that republishes identical hits, an
    /// icon batch that only fills the cache) must not reset the highlight the
    /// user is moving with the arrow keys.
    pub fn set_results<C>(
        &self,
        items: Vec<ResultItem>,
        icons: Vec<Option<Arc<Image>>>,
        cx: &mut Context<C>,
    ) {
        self.set_results_with_default(items, icons, None, cx);
    }

    /// Replace the displayed rows like [`Self::set_results`], but hand the
    /// highlight to `default_selection` whenever the row list changed.
    ///
    /// The launcher uses this to land the highlight on the row a keystroke just
    /// produced: the built-in "File Search" command when its keyword was typed,
    /// or the first application on the home page, whose command row is pinned on
    /// top for discoverability but must not steal "summon + Enter". `None` keeps
    /// the plain rule: the old index stays while it still points at a row, and a
    /// shorter list falls back to the first row.
    pub fn set_results_with_default<C>(
        &self,
        items: Vec<ResultItem>,
        icons: Vec<Option<Arc<Image>>>,
        default_selection: Option<usize>,
        cx: &mut Context<C>,
    ) {
        self.state.update(cx, |this, cx| {
            let rows_changed = this.items != items;
            let icons_changed = !icons_equal(&this.icons, &icons);
            if !rows_changed && !icons_changed {
                return;
            }
            this.items = items;
            this.icons = icons;
            if rows_changed {
                this.selected = next_selection(this.selected, this.items.len(), default_selection);
            }
            cx.notify();
        });
    }

    /// Number of results from the latest update (used for window sizing).
    pub fn visible_count(&self, cx: &App) -> usize {
        self.state.read(cx).items.len()
    }

    /// Allow or refuse confirmation of the displayed rows.
    ///
    /// The launcher keeps the previous rows on screen while a new search runs
    /// (blanking them made the drop-down flash on every keystroke), so for that
    /// window the rows are visible but must not be actionable: confirming one
    /// would launch something the input no longer names.
    pub fn set_confirmable<C: AppContext>(&self, confirmable: bool, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            if this.confirmable != confirmable {
                this.confirmable = confirmable;
                cx.notify();
            }
        });
    }

    /// Replace the type label shown on the right of every row. Used when the
    /// UI language changes at runtime — the label is stored state, so it does
    /// not re-translate on its own.
    pub fn set_type_label<C: AppContext>(&self, label: impl Into<String>, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.type_label = label.into();
            cx.notify();
        });
    }

    /// Move selection by `delta` rows (negative moves up), clamping to bounds.
    pub fn select_relative<C>(
        &self,
        delta: i32,
        _window: &mut gpui::Window,
        cx: &mut Context<C>,
    ) -> Option<usize> {
        let mut next = None;
        self.state.update(cx, |this, cx| {
            if !this.items.is_empty() {
                // The first press selects the first row (not the second):
                // treat "nothing selected" as -1 so `+1` lands on index 0.
                let current = this.selected.map(|i| i as i32).unwrap_or(-1);
                let index = (current + delta).clamp(0, this.items.len() as i32 - 1) as usize;
                this.selected = Some(index);
                next = Some(index);
                cx.notify();
            }
        });
        next
    }

    /// Confirm the currently selected row, invoking the delegate's `on_confirm`
    /// callback. Returns whether the launcher should hide afterwards (no-op
    /// when nothing is selected: `false`).
    pub fn confirm_selected<C>(&self, _window: &mut gpui::Window, cx: &mut Context<C>) -> bool {
        let mut should_hide = false;
        self.state.update(cx, |this, cx| {
            if !this.confirmable {
                return;
            }
            if let Some(index) = this.selected {
                if index < this.items.len() {
                    if let Some(cb) = this.on_confirm.clone() {
                        should_hide = cb(index, cx);
                    }
                }
            }
            cx.notify();
        });
        should_hide
    }

    /// The currently selected row, cloned (or `None` when nothing is selected).
    /// Used by panel action bars to know which item a view-level action targets.
    pub fn selected_item(&self, cx: &App) -> Option<ResultItem> {
        let state = self.state.read(cx);
        state
            .selected
            .and_then(|index| state.items.get(index).cloned())
    }

    /// Guarantee a selection exists: if nothing is selected but rows are
    /// present, select the first and return whether there is a selectable row.
    /// Used by the panel's Enter handler so confirming never silently no-ops.
    pub fn ensure_selected<C>(&self, cx: &mut C) -> bool
    where
        C: gpui::AppContext,
    {
        let mut ok = false;
        self.state.update(cx, |this, cx| {
            if this.selected.is_none() && !this.items.is_empty() {
                this.selected = Some(0);
            }
            ok = this.selected.is_some() && !this.items.is_empty();
            cx.notify();
        });
        ok
    }

    /// Render the scrollable list element, capped at `max_height` so rows
    /// beyond the visible drop-down can be scrolled into view. `selected_wash`
    /// is the white selection-wash opacity for the current frame (the app
    /// adapts it to the backdrop's brightness, so it is pushed in like
    /// `max_height` rather than read from the global theme).
    pub fn render<C>(
        &self,
        max_height: f32,
        selected_wash: f32,
        cx: &mut Context<C>,
    ) -> impl IntoElement {
        self.state.update(cx, |this, _| {
            this.max_height = max_height;
            this.selected_wash = selected_wash;
        });
        div()
            .id(ElementId::from("results-list"))
            .h(px(max_height))
            .child(self.state.clone())
    }
}
