//! Compact launcher result list built from NSH host elements.

use nsh_core::ui::{prelude::*, px, rgb};

use crate::palette;

const ROW_HEIGHT: f32 = 42.0;
const ICON_SIZE: f32 = 24.0;
const DETAIL_WIDTH: f32 = 240.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherRow {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
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
            rows.with(|rows| rows[index].clone())
        });
        let has_icon = move || row.with(|row| row.icon.is_some());
        let icon_src = move || row.with(|row| row.icon.clone().unwrap_or_default());
        let activate = on_activate.clone();

        let mut result = view()
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
                view().flex_1().min_w(px(0.0)).child(
                    text(move || row.with(|row| row.title.clone()))
                        .text_size(px(15.0))
                        .text_color(rgb(palette::FOREGROUND))
                        .number_of_lines(1),
                ),
            )
            .child(
                view().w(px(DETAIL_WIDTH)).flex_shrink_0().child(
                    text(move || row.with(|row| row.subtitle.clone()))
                        .text_size(px(11.0))
                        .text_color(rgb(palette::MUTED_FOREGROUND))
                        .text_right()
                        .number_of_lines(1),
                ),
            )
    })
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
