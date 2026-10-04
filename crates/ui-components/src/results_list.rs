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

use std::{cell::RefCell, rc::Rc, sync::Arc};

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

/// Observer for confirmation attempts (see [`set_confirm_trace`]).
pub type ConfirmTrace = Rc<dyn Fn(&str)>;

thread_local! {
    /// The installed [`ConfirmTrace`], if any. The host app installs one when
    /// its diagnostics are on.
    static CONFIRM_TRACE: RefCell<Option<ConfirmTrace>> = const { RefCell::new(None) };
}

/// Install an observer for every confirmation attempt, accepted or refused.
///
/// The host app sets this when its diagnostics are on, so a key that appears to
/// do nothing can say *why*: rows that are not confirmable, a selection past the
/// end, no callback. The widget never prints on its own.
pub fn set_confirm_trace(trace: Option<ConfirmTrace>) {
    CONFIRM_TRACE.with(|slot| *slot.borrow_mut() = trace);
}

fn trace_confirm(message: &str) {
    CONFIRM_TRACE.with(|slot| {
        if let Some(trace) = slot.borrow().as_ref() {
            trace(message);
        }
    });
}

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
    /// A filesystem directory offered to an open/save dialog.
    Directory {
        path: std::path::PathBuf,
        title: String,
        subtitle: String,
    },
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
/// label, a plugin row's subtitle). Fixed, and applied on every row, so the text
/// starts on one axis and the column's edge lands in one place instead of
/// drifting with the length of each string.
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
    /// Whether the displayed rows may be confirmed. The directory picker turns
    /// this off while a search is in flight, so rows kept on screen for a smooth
    /// repaint cannot be navigated to by mistake. See [`ResultList::set_confirmable`].
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
        ResultItem::Directory { path, .. } => ElementId::from(path.to_string_lossy().into_owned()),
        ResultItem::Action { .. } => ElementId::from(format!("result-action-{index}")),
        ResultItem::Link { .. } => ElementId::from(format!("result-link-{index}")),
        ResultItem::Plugin { .. } => ElementId::from(format!("result-plugin-{index}")),
        ResultItem::Calendar { .. } => ElementId::from(format!("result-calendar-{index}")),
        ResultItem::File { path, .. } => ElementId::from(path.to_string_lossy().into_owned()),
        ResultItem::Command { .. } => ElementId::from(format!("result-command-{index}")),
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
        // A directory picker row is the whole path and nothing else: the path is
        // what Enter navigates to, so a separate name and parent column would
        // only ask the user to reassemble it by eye.
        ResultItem::Directory { path, .. } => {
            (row, path.to_string_lossy().into_owned(), None, None, None)
        }
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
    // A row with nothing to put in them (a picker's path) drops them entirely
    // instead of reserving two empty columns against its own text.
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
        self.state.update(cx, |this, cx| {
            let rows_changed = this.items != items;
            let icons_changed = !icons_equal(&this.icons, &icons);
            if !rows_changed && !icons_changed {
                return;
            }
            this.items = items;
            this.icons = icons;
            if rows_changed {
                // Default-select the first row so Enter (or the highlight) works
                // immediately after typing, without a manual Down press. Keep the
                // old selection when it still points at a row.
                if this
                    .selected
                    .is_none_or(|selected| selected >= this.items.len())
                {
                    this.selected = (!this.items.is_empty()).then_some(0);
                }
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
    /// The directory picker keeps the previous rows on screen while it searches
    /// for the new query (blanking them made the drop-down flash on every
    /// keystroke), so for that window the rows are visible but must not be
    /// actionable: confirming one would navigate to a folder the user did not
    /// ask for.
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
    ///
    /// Every refusal is reported through [`set_confirm_trace`]. "Nothing
    /// happened when I pressed the key" is otherwise indistinguishable from a
    /// broken keybinding, and the reason — the rows were stale, or nothing was
    /// selected — is exactly what tells the two apart.
    pub fn confirm_selected<C>(&self, _window: &mut gpui::Window, cx: &mut Context<C>) -> bool {
        let mut should_hide = false;
        self.state.update(cx, |this, cx| {
            if !this.confirmable {
                trace_confirm("refused: the displayed rows are not confirmable (stale)");
                return;
            }
            match this.selected {
                None => trace_confirm("refused: nothing is selected"),
                Some(index) if index >= this.items.len() => trace_confirm(&format!(
                    "refused: selected row {index} is past the end ({} rows)",
                    this.items.len()
                )),
                Some(index) => match this.on_confirm.clone() {
                    Some(cb) => {
                        trace_confirm(&format!("confirmed row {index}"));
                        should_hide = cb(index, cx);
                    }
                    None => trace_confirm("refused: no confirm callback is registered"),
                },
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
