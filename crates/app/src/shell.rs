//! Native NSH Rust launcher root. This surface never starts QuickJS.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nsh_core::keymap::When;
use nsh_core::ui::{prelude::*, px, rgb};
use steward_ui_components::{result_list, LauncherRow};

use crate::services::{services, AppServices, RowAction, WindowCommand};

const HEADER_HEIGHT: f32 = 60.0;
const MAX_VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 42.0;

#[component]
pub fn StewardLauncher() -> impl IntoElement {
    let app: Arc<AppServices> = services().expect("launcher AppServices in EngineContext");
    let query = signal(String::new());
    let rows = signal(Vec::<LauncherRow>::new());
    let actions = signal(Vec::<RowAction>::new());
    let selected = signal(0_usize);
    let first_visible = signal(0_usize);
    let generation = signal(0_u64);
    let input_ref = element_ref();
    let list_ref = element_ref();

    let filter: Rc<dyn Fn(&str)> = {
        let app = app.clone();
        Rc::new(move |text: &str| {
            query.set(text.to_owned());
            let next = generation.with_untracked(|generation| *generation + 1);
            generation.set(next);
            let app = app.clone();
            let query_text = text.to_owned();
            spawn(async move {
                if let Some(reply) = app.search(next, query_text).await {
                    if generation.get() == reply.generation {
                        let visible_rows = reply.rows.len().min(MAX_VISIBLE_ROWS);
                        let height = HEADER_HEIGHT + 8.0 + visible_rows as f32 * ROW_HEIGHT;
                        rows.set(reply.rows);
                        actions.set(reply.actions);
                        selected.set(0);
                        first_visible.set(0);
                        list_ref.scroll_to_index(0, percentage(0.0), px(0.0), false);
                        app.send_window_command(WindowCommand::Resize { height });
                    }
                }
            });
        })
    };
    filter("");

    let observed_revision = signal(app.search_revision());
    set_interval(Duration::from_millis(250), {
        let app = app.clone();
        let filter = filter.clone();
        move || {
            let revision = app.search_revision();
            if revision != observed_revision.get_untracked() {
                observed_revision.set(revision);
                filter(&query.get_untracked());
            }
        }
    });

    let activate: Rc<dyn Fn(usize)> = {
        let app = app.clone();
        Rc::new(move |index: usize| {
            let action = actions.with_untracked(|actions| actions.get(index).cloned());
            if let Some(action) = action {
                app.send_window_command(WindowCommand::Activate(action));
            }
        })
    };
    let submit: Rc<dyn Fn(&str)> = {
        let activate = activate.clone();
        Rc::new(move |_: &str| {
            activate(selected.get_untracked());
        })
    };
    let select_relative: Rc<dyn Fn(isize)> = {
        Rc::new(move |delta: isize| {
            let len = rows.with_untracked(Vec::len);
            let Some(next) = next_selection(selected.get_untracked(), len, delta) else {
                return;
            };
            selected.set(next);
            let first = first_visible.get_untracked();
            if next < first {
                list_ref.scroll_to_index(next as u32, percentage(0.0), px(0.0), false);
            } else if next >= first + MAX_VISIBLE_ROWS {
                list_ref.scroll_to_index(next as u32, percentage(1.0), px(0.0), false);
            }
        })
    };
    let key_handler: Rc<dyn Fn(&str)> = {
        let app = app.clone();
        let select_relative = select_relative.clone();
        Rc::new(move |key: &str| match key {
            "up" | "ArrowUp" => {
                select_relative(-1);
            }
            "down" | "ArrowDown" => {
                select_relative(1);
            }
            "escape" | "Escape" => {
                app.send_window_command(WindowCommand::Hide);
            }
            _ => {}
        })
    };

    bind_launcher_key("up", true, key_handler.clone());
    bind_launcher_key("down", true, key_handler.clone());
    bind_launcher_key("escape", false, key_handler);

    add_input_focus(&input_ref);
    let has_rows = move || rows.with(|rows| !rows.is_empty());

    view()
        .size_full()
        .p(px(4.0))
        .bg(rgb(steward_ui_components::palette::BACKGROUND)
            .opacity(steward_ui_components::palette::SCRIM_ALPHA))
        .child(
            view()
                .size_full()
                .flex_col()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(HEADER_HEIGHT))
                        .px_3()
                        .flex_row()
                        .items_center()
                        .child(
                            input()
                                .element_ref(&input_ref)
                                .flex_1()
                                .bg(rgba(0x00000000))
                                .value(move || query.get())
                                .placeholder(move || {
                                    if app.locale().starts_with("zh") {
                                        "搜索应用和命令"
                                    } else {
                                        "Search apps or type a command..."
                                    }
                                })
                                .on_change({
                                    let filter = filter.clone();
                                    move |text: &str| filter(text)
                                })
                                .on_submit({
                                    let submit = submit.clone();
                                    move |text: &str| submit(text)
                                })
                                .text_size(px(16.0))
                                .text_color(rgb(steward_ui_components::palette::FOREGROUND))
                                .placeholder_color(rgb(
                                    steward_ui_components::palette::MUTED_FOREGROUND,
                                ))
                                .selection_color(rgb(steward_ui_components::palette::ACCENT)),
                        ),
                )
                .when(has_rows(), |this| {
                    this.child(view().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.04)))
                })
                .child(view().flex_1().child(result_list(
                    rows,
                    selected,
                    &list_ref,
                    first_visible,
                    {
                        let activate = activate.clone();
                        move |index| activate(index)
                    },
                ))),
        )
}

fn bind_launcher_key(key: &'static str, repeat: bool, handler: Rc<dyn Fn(&str)>) {
    on_key_with(
        key,
        KeyOptions {
            when: When::Always,
            repeat,
        },
        move |key| handler(key),
    );
}

fn next_selection(current: usize, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some((current.min(len - 1) as isize + delta).clamp(0, len as isize - 1) as usize)
}

fn add_input_focus(input: &nsh_core::ui::ElementRef) {
    set_timeout(Duration::from_millis(10), {
        let input = *input;
        move || input.focus()
    });
}

#[cfg(test)]
mod tests {
    use super::next_selection;

    #[test]
    fn selection_moves_and_clamps_to_the_result_bounds() {
        assert_eq!(next_selection(0, 3, 1), Some(1));
        assert_eq!(next_selection(2, 3, 1), Some(2));
        assert_eq!(next_selection(2, 3, -1), Some(1));
        assert_eq!(next_selection(0, 3, -1), Some(0));
        assert_eq!(next_selection(9, 3, 1), Some(2));
        assert_eq!(next_selection(0, 0, 1), None);
    }
}
