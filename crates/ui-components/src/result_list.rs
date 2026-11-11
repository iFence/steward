//! Compact launcher result list built from NSH host elements.

use nsh_core::ui::{prelude::*, px, rgb};

use crate::palette;

const ROW_HEIGHT: f32 = 42.0;
const ICON_SIZE: f32 = 24.0;
const TITLE_MAX_WIDTH: f32 = 280.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherRow {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub subtitle_placement: SubtitlePlacement,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SubtitlePlacement {
    #[default]
    Trailing,
    AfterTitle,
}

impl LauncherRow {
    pub fn with_subtitle_after_title(mut self) -> Self {
        self.subtitle_placement = SubtitlePlacement::AfterTitle;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressedKey {
    Chrome,
}

pub fn result_list(
    rows: Signal<Vec<LauncherRow>>,
    selected: Signal<usize>,
    list_ref: &ElementRef,
    first_visible: Signal<usize>,
    on_activate: impl Fn(usize) + Clone + 'static,
) -> impl IntoElement {
    let count = move || rows.with(Vec::len);
    virtual_list(count, move |at| {
        // A virtual-list cell is reused for another row when the list scrolls.
        // Read Position::index inside each reactive binding so the row data and
        // selection highlight follow the cell's current index.
        let row = computed(move || {
            let index = at.index();
            rows.with(|rows| row_at(rows, index))
        });
        let has_icon = move || row.with(|row| row.icon.is_some());
        let icon_src = move || row.with(|row| row.icon.clone().unwrap_or_default());
        let subtitle_is_trailing =
            move || row.with(|row| row.subtitle_placement == SubtitlePlacement::Trailing);
        let activate = on_activate.clone();

        let mut result = view()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_3()
            .flex_row()
            .items_center()
            .border_b(px(1.0))
            .border_color(rgb(palette::FOREGROUND).opacity(0.04))
            .bg(move || {
                if selected.get() == at.index() {
                    rgb(palette::ROW_BACKGROUND_SELECTED)
                } else {
                    rgba(0x00000000)
                }
            })
            .on_press(move || {
                let index = at.index();
                selected.set(index);
                activate(index);
            });
        result = result.child(when(has_icon, move || {
            image(icon_src)
                .w(px(ICON_SIZE))
                .h(px(ICON_SIZE))
                .flex_shrink_0()
        }));
        result
            .child(
                view()
                    .min_w(px(0.0))
                    .max_w(px(TITLE_MAX_WIDTH))
                    .flex_shrink(1.0)
                    .child(
                        text(move || row.with(|row| row.title.clone()))
                            .text_size(px(15.0))
                            .text_color(rgb(palette::FOREGROUND))
                            .number_of_lines(1),
                    ),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .style_when(subtitle_is_trailing, |style| style.justify_end())
                    .child(
                        text(move || row.with(|row| row.subtitle.clone()))
                            .text_size(px(12.0))
                            .text_color(rgb(palette::RESULT_LABEL_FOREGROUND))
                            .number_of_lines(1),
                    ),
            )
    })
    .keys(move || rows.with(|rows| rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>()))
    .estimated_item_size(px(ROW_HEIGHT))
    .on_viewable_items_change(move |viewable: &ViewableItems| {
        if let Some((first, _)) = viewable.viewable {
            first_visible.set(first);
        }
    })
    .element_ref(list_ref)
    .flex_1()
    .min_h(px(0.0))
    .w_full()
}

/// A recycled cell can outlive the result that filled it for one commit.
/// Return an empty row rather than indexing an empty result list.
fn row_at(rows: &[LauncherRow], index: usize) -> LauncherRow {
    rows.get(index).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nsh_core::ui::{prelude::*, Root};
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;

    use super::{result_list, row_at, LauncherRow};

    #[test]
    fn an_out_of_range_result_row_is_empty() {
        let row = row_at(&[], 0);
        assert!(row.id.is_empty());
        assert!(row.title.is_empty());
        assert!(row.subtitle.is_empty());
        assert_eq!(row.icon, None);
    }

    #[component]
    fn ResultsThatBecomeEmpty() -> impl IntoElement {
        let rows = signal(Vec::<LauncherRow>::new());
        let selected = signal(0);
        let first_visible = signal(0);
        let list_ref = element_ref();

        set_timeout(Duration::from_millis(50), move || {
            rows.set(vec![LauncherRow {
                id: "first".into(),
                title: "first".into(),
                subtitle: "64".into(),
                icon: None,
                subtitle_placement: Default::default(),
            }]);
        });
        set_timeout(Duration::from_millis(100), move || rows.set(Vec::new()));
        set_timeout(Duration::from_millis(150), move || {
            rows.set(vec![LauncherRow {
                id: "restored".into(),
                title: "restored".into(),
                subtitle: "64".into(),
                icon: None,
                subtitle_placement: Default::default(),
            }]);
        });

        result_list(rows, selected, &list_ref, first_visible, |_| {})
    }

    #[test]
    fn results_reappear_after_an_empty_update() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ResultsThatBecomeEmpty>())).unwrap();
        app.wait_for("restored").unwrap();
    }
}
