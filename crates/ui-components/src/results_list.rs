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
/// The first row reachable with a Ctrl+number shortcut. The top row is
/// confirmed with a bare Enter, so the digits start at the second row (Ctrl+1)
/// and run to Ctrl+7 for the eighth and last visible row.
const FIRST_CTRL_INDEX: usize = 1;
/// The label on the top row's cap: the key that confirms it. Not localized —
/// "Enter" is the name printed on the key of every keyboard and keyboard
/// layout Steward supports, so it reads the same in all seven locales.
const ENTER_KEY_LABEL: &str = "Enter";
/// Width reserved for the shortcut before every row's content, so that the key
/// caps, icons and names line up across rows. Sized for the widest cap — a
/// five-glyph shortcut such as the German `Strg+1` — plus a gap to the icon.
const HINT_COLUMN_WIDTH: f32 = 52.0;
/// Horizontal padding inside the chip, between its border and the key text.
const HINT_CHIP_PADDING: f32 = 5.0;
/// Width of the secondary text column (a file row's folder, an app row's kind
/// label, a plugin row's subtitle). Fixed, and applied on every row, so the text
/// starts on one axis and the column's edge lands in one place instead of
/// drifting with the length of each string.
const DETAIL_COLUMN_WIDTH: f32 = 280.0;
/// Width reserved for the size column. Fixed — and reserved on every row, file
/// or not — so the trailing columns line up down the whole drop-down.
const SIZE_COLUMN_WIDTH: f32 = 64.0;
/// Opacity of the chip's border. The border is `palette::BORDER` (white 0.20
/// over the launcher surface) dropped to a whisper: at full strength it reads
/// as a box drawn around every row rather than as a key cap.
const HINT_CHIP_BORDER_ALPHA: f32 = 0.45;

/// The row confirmed by the bare Enter key (and by a click): the first one.
/// Every visible row therefore advertises a key — Enter on top, Ctrl+1 …
/// Ctrl+7 below it — and no row is left without one.
pub fn enter_row_index() -> usize {
    0
}

/// The Ctrl+number shortcut that confirms the row at `index`, or `None` for the
/// rows that have none. The top row belongs to Enter (see [`enter_row_index`]);
/// the rows below it take Ctrl+1, Ctrl+2, ... in order, capped at the last
/// visible row.
pub(crate) fn shortcut_key_for(index: usize) -> Option<char> {
    let digit = index.checked_sub(FIRST_CTRL_INDEX)?.checked_add(1)?;
    (digit <= VISIBLE_ROWS - FIRST_CTRL_INDEX)
        .then(|| char::from_digit(digit as u32, 10))
        .flatten()
}

/// The text of the keyboard shortcut for the row at `index`: `Enter` on the top
/// row, `Ctrl+1` … `Ctrl+7` on the rows below it, and `None` for a row past the
/// last mapped key. `modifier` is the localized name of the Ctrl key; an empty
/// one (the hint label is missing from the active locale) falls back to the
/// bare number.
pub(crate) fn shortcut_hint(index: usize, modifier: &str) -> Option<String> {
    if index == enter_row_index() {
        return Some(ENTER_KEY_LABEL.to_owned());
    }
    let key = shortcut_key_for(index)?;
    Some(if modifier.is_empty() {
        key.to_string()
    } else {
        format!("{modifier}+{key}")
    })
}

/// The row index that a digit shortcut selects when combined with Ctrl, i.e.
/// the inverse of [`shortcut_hint`]. `None` for digits outside the mapped
/// range. Kept next to the forward mapping so the two cannot drift apart.
pub fn shortcut_digit_index(digit: char) -> Option<usize> {
    let value = digit.to_digit(10)? as usize;
    if value == 0 || value > VISIBLE_ROWS - FIRST_CTRL_INDEX {
        return None;
    }
    Some(FIRST_CTRL_INDEX + value - 1)
}

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
    /// Localized name of the Ctrl key ("Ctrl" / "^"), shown in the shortcut cap
    /// on every row below the first. Empty drops the modifier and leaves the
    /// bare digit.
    shortcut_modifier: String,
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
        let modifier = self.shortcut_modifier.clone();
        let rows = self.items[range.clone()]
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let index = range.start + offset;
                let shortcut = match item {
                    // A placeholder row is not confirmable, so it gets no
                    // shortcut: the hint never advertises a key that no-ops.
                    ResultItem::Loading { .. } => None,
                    _ => shortcut_hint(index, &modifier),
                };
                render_row(
                    item,
                    self.icons.get(index).cloned().flatten(),
                    &type_label,
                    shortcut,
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

    #[test]
    fn every_visible_row_advertises_a_key() {
        // The top row is the bare Enter target, so it shows "Enter" rather than
        // a digit...
        assert_eq!(enter_row_index(), 0);
        assert_eq!(
            shortcut_hint(enter_row_index(), "Ctrl").as_deref(),
            Some("Enter")
        );
        assert_eq!(shortcut_key_for(enter_row_index()), None);
        // ...and Ctrl+1 upward starts on the row below it.
        assert_eq!(shortcut_key_for(1), Some('1'));
        assert_eq!(shortcut_key_for(2), Some('2'));
        assert_eq!(shortcut_key_for(7), Some('7'));
        assert_eq!(shortcut_hint(1, "Ctrl").as_deref(), Some("Ctrl+1"));
        assert_eq!(shortcut_hint(7, "Ctrl").as_deref(), Some("Ctrl+7"));
        // Nothing past the last visible row: the drop-down renders exactly
        // `VISIBLE_ROWS` rows, so a hint there would advertise a dead key.
        assert_eq!(shortcut_key_for(VISIBLE_ROWS), None);
        assert_eq!(shortcut_hint(VISIBLE_ROWS, "Ctrl"), None);
        // Enter is not localized: it names the key printed on every keyboard.
        assert_eq!(shortcut_hint(0, "").as_deref(), Some("Enter"));
    }

    #[test]
    fn shortcut_hints_carry_the_localized_modifier() {
        assert_eq!(shortcut_hint(1, "Ctrl").as_deref(), Some("Ctrl+1"));
        assert_eq!(shortcut_hint(7, "^").as_deref(), Some("^+7"));
        assert_eq!(shortcut_hint(1, "Strg").as_deref(), Some("Strg+1"));
        // A locale without the modifier label still shows the key itself.
        assert_eq!(shortcut_hint(1, "").as_deref(), Some("1"));
    }

    #[test]
    fn digits_map_back_to_the_rows_the_hints_advertise() {
        // The digit handler and the hint renderer must agree: every advertised
        // shortcut selects the row that shows it.
        for index in 0..=VISIBLE_ROWS {
            let Some(key) = shortcut_key_for(index) else {
                continue;
            };
            assert_eq!(shortcut_digit_index(key), Some(index));
        }
    }

    #[test]
    fn unmapped_digits_do_not_select_a_row() {
        // Ctrl+0 is not a shortcut (the row numbering starts at 1), and
        // neither is a digit past the last visible row.
        assert_eq!(shortcut_digit_index('0'), None);
        assert_eq!(shortcut_digit_index('8'), None);
        assert_eq!(shortcut_digit_index('9'), None);
        assert_eq!(shortcut_digit_index('a'), None);
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
///
/// A row with a keyboard shortcut (see [`shortcut_hint`]) also carries the
/// hint "Ctrl+N" as a fixed-width cell before its right-hand detail column, so
/// a long name is truncated ahead of it rather than running into the shortcut.
#[allow(clippy::too_many_arguments)]
fn render_row(
    item: &ResultItem,
    icon: Option<Arc<Image>>,
    type_label: &str,
    // Already-formatted shortcut hint ("Ctrl+1"), or `None` for a row without
    // one (the first row, and any row past the last mapped digit).
    shortcut: Option<String>,
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
    // the text out here is what lets the shortcut hint sit at the head of the row,
    // and the trailing columns stay put, without repeating the layout in every
    // arm. `None` means the row has that column and leaves it out entirely.
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

    // The row reads left to right: the key cap (or the empty spacer that
    // reserves its width on the first row), the icon, the name, the size, then
    // the secondary text.
    //
    // Both text columns truncate to the right and are laid out left to right, so
    // the name gives way to the size and the size never moves: the name is the
    // flexible column and the two trailing ones are reserved on every row that
    // has them, which is what keeps the columns on one axis down the drop-down.
    // A row with nothing to put in them (a picker's path) drops them entirely
    // instead of reserving two empty columns against its own text.
    let mut row = row
        .child(hint_cell(shortcut))
        .when_some(icon, |this, icon| {
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

/// The shortcut cell that opens every row: the key itself ("Ctrl+1") drawn in
/// a small rounded box, the way launchers and menus show a key cap, inside a
/// fixed-width column.
///
/// The shortcut leads the row rather than trailing it, which keeps the two out
/// of each other's way without any overlay: a truncated long name can never
/// reach the key, and the key never shifts between rows. The first row has no
/// shortcut (see [`shortcut_key_for`]), so it gets the same fixed-width cell
/// with nothing in it and every row's icon and name stay on one axis.
fn hint_cell(hint: Option<String>) -> gpui::Div {
    let cell = div()
        .w(px(HINT_COLUMN_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center();
    let Some(hint) = hint else {
        return cell;
    };
    cell.child(
        div()
            .px(px(HINT_CHIP_PADDING))
            .rounded_md()
            // The row has no background of its own (the window root paints one
            // translucent scrim across the whole launcher), so the cap is a
            // raised surface tint plus a hairline border rather than a fill
            // that would have to match the backdrop.
            .bg(rgb(crate::palette::BACKGROUND_ALT))
            .border_1()
            .border_color(rgb(crate::palette::BORDER).opacity(HINT_CHIP_BORDER_ALPHA))
            .text_color(rgb(crate::palette::FOREGROUND))
            .text_size(px(11.0))
            .child(hint),
    )
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
            shortcut_modifier: String::new(),
            confirmable: true,
        });
        Self { state }
    }

    /// Set the localized name of the Ctrl key shown in the row shortcut hints
    /// ("Ctrl" / "^"). An empty string hides the hints (the list is still
    /// keyboard-selectable; only the on-screen affordance goes away).
    pub fn set_shortcut_modifier<C: AppContext>(&self, modifier: impl Into<String>, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            let modifier = modifier.into();
            if this.shortcut_modifier != modifier {
                this.shortcut_modifier = modifier;
                cx.notify();
            }
        });
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

    /// Select the row at `index` outright (used by the Ctrl+number shortcuts,
    /// which jump straight to a row instead of stepping the selection). Returns
    /// whether the index names an existing row, leaving the selection alone
    /// when it does not.
    pub fn set_selected<C: AppContext>(&self, index: usize, cx: &mut C) -> bool {
        let mut ok = false;
        self.state.update(cx, |this, cx| {
            if index < this.items.len() {
                ok = true;
                if this.selected != Some(index) {
                    this.selected = Some(index);
                    cx.notify();
                }
            }
        });
        ok
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
