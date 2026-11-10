//! Native NSH Rust launcher root. This surface never starts QuickJS.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nsh_core::keymap::When;
use nsh_core::ui::{prelude::*, px, rgb};
use steward_ui_components::{result_list, LauncherRow};

use crate::config::{
    LAUNCHER_HEADER_HEIGHT, LAUNCHER_INPUT_HEIGHT, LAUNCHER_INPUT_INSET, LAUNCHER_MARGIN,
    LAUNCHER_STATUS_BAR_HEIGHT,
};
use crate::services::{services, LauncherServices, RowAction, WindowCommand};

const MAX_VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 42.0;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(40);
/// Keep reasserting the field across the activation/IME hand-off instead of
/// relying on one focus command landing on the right frame.
const FOCUS_RETRIES: u8 = 5;

#[component]
pub fn StewardLauncher() -> impl IntoElement {
    let app: Arc<LauncherServices> = services().expect("launcher services in EngineContext");
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
            set_timeout(SEARCH_DEBOUNCE, move || {
                if generation.get_untracked() != next {
                    return;
                }
                spawn(async move {
                    if let Some(reply) = app.search(next, query_text).await {
                        if generation.get() == reply.generation {
                            let mut rows_value = reply.rows;
                            let mut actions_value = reply.actions;
                            if let Some(notice) = app.notice() {
                                rows_value.insert(
                                    0,
                                    LauncherRow {
                                        id: "launch-notice".into(),
                                        title: notice.title,
                                        subtitle: notice.message,
                                        icon: None,
                                    },
                                );
                                actions_value.insert(0, RowAction::DismissNotice);
                            }
                            let height = launcher_height(rows_value.len().min(MAX_VISIBLE_ROWS));
                            rows.set(rows_value);
                            actions.set(actions_value);
                            selected.set(0);
                            first_visible.set(0);
                            list_ref.scroll_to_index(0, percentage(0.0), px(0.0), false);
                            app.send_window_command(WindowCommand::Resize { height });
                        }
                    }
                });
            });
        })
    };
    filter("");

    let observed_revision = signal(app.search_revision());
    let observed_locale = signal(app.locale_revision());
    let observed_launcher_reset = signal(app.launcher_reset_revision());
    let observed_launcher_focus = signal(app.launcher_focus_revision());
    let focus_retries = signal(0_u8);
    let scrim_alpha = signal(app.launcher_scrim_alpha());
    set_interval(Duration::from_millis(250), {
        let app = app.clone();
        let filter = filter.clone();
        move || {
            let revision = app.search_revision();
            let locale_revision = app.locale_revision();
            let search_changed = revision != observed_revision.get_untracked();
            let locale_changed = locale_revision != observed_locale.get_untracked();
            if search_changed {
                observed_revision.set(revision);
            }
            if locale_changed {
                observed_locale.set(locale_revision);
            }
            if search_changed || locale_changed {
                filter(&query.get_untracked());
            }
        }
    });
    set_interval(Duration::from_millis(20), {
        let app = app.clone();
        let filter = filter.clone();
        move || {
            let reset_revision = app.launcher_reset_revision();
            let focus_revision = app.launcher_focus_revision();
            let launcher_scrim_alpha = app.launcher_scrim_alpha();
            let reset_changed = reset_revision != observed_launcher_reset.get_untracked();
            let focus_changed = focus_revision != observed_launcher_focus.get_untracked();
            if reset_changed {
                observed_launcher_reset.set(reset_revision);
                filter("");
            }
            if focus_changed {
                observed_launcher_focus.set(focus_revision);
            }
            if launcher_scrim_alpha != scrim_alpha.get_untracked() {
                scrim_alpha.set(launcher_scrim_alpha);
            }
            if reset_changed || focus_changed {
                input_ref.focus();
                focus_retries.set(FOCUS_RETRIES);
            } else if focus_retries.get_untracked() > 0 {
                focus_retries.update(|remaining| *remaining -= 1);
                input_ref.focus();
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
        let app = app.clone();
        Rc::new(move |text: &str| {
            let action = actions.with_untracked(|actions| {
                submission_action(actions, selected.get_untracked(), text)
            });
            if let Some(action) = action {
                app.send_window_command(WindowCommand::Activate(action));
            }
        })
    };
    let select_relative: Rc<dyn Fn(isize)> = {
        Rc::new(move |delta: isize| {
            let len = rows.with_untracked(Vec::len);
            if len == 0 {
                return;
            }
            let Some(next) = next_selection(selected.get_untracked(), len, delta) else {
                // At either boundary the selection is already correct, but
                // the list itself can still be offset by a pending or native
                // scroll. Pin the boundary row back into view.
                let current = selected.get_untracked().min(len - 1);
                let position = if delta < 0 { 0.0 } else { 1.0 };
                selected.update(|selection| *selection = current);
                list_ref.scroll_to_index(current as u32, percentage(position), px(0.0), false);
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
        .py(px(LAUNCHER_MARGIN))
        .window_drag(true)
        .bg(rgb(steward_ui_components::palette::BACKGROUND).opacity(scrim_alpha.get()))
        .child(
            view()
                .size_full()
                .flex_col()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(LAUNCHER_HEADER_HEIGHT))
                        .px(px(LAUNCHER_INPUT_INSET))
                        .flex_row()
                        .items_center()
                        .window_drag(true)
                        .child(
                            input()
                                .element_ref(&input_ref)
                                .flex_1()
                                .h(px(LAUNCHER_INPUT_HEIGHT))
                                .bg(rgba(0x00000000))
                                .value(move || query.get())
                                .placeholder({
                                    let app = app.clone();
                                    move || {
                                        let _ = observed_locale.get();
                                        app.search_placeholder()
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
                                .placeholder_color(
                                    rgb(steward_ui_components::palette::FOREGROUND).opacity(
                                        steward_ui_components::palette::PLACEHOLDER_OPACITY,
                                    ),
                                )
                                .selection_color(rgb(steward_ui_components::palette::ACCENT)),
                        ),
                )
                .when(has_rows(), |this| {
                    this.child(view().w_full().h(px(1.0)).bg(rgb(0xffffff).opacity(0.04)))
                })
                .child(view().flex_1().window_drag(false).child(result_list(
                    rows,
                    selected,
                    &list_ref,
                    first_visible,
                    {
                        let activate = activate.clone();
                        move |index| activate(index)
                    },
                )))
                .child(
                    view()
                        .h(px(LAUNCHER_STATUS_BAR_HEIGHT))
                        .w_full()
                        .px(px(LAUNCHER_INPUT_INSET))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .border_t(px(1.0))
                        .border_color(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.06))
                        .window_drag(false)
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(status_hint("↑↓", {
                                    let app = app.clone();
                                    move || {
                                        let _ = observed_locale.get();
                                        app.ui_strings().launcher_navigate
                                    }
                                }))
                                .child(status_hint("↵", {
                                    let app = app.clone();
                                    move || {
                                        let _ = observed_locale.get();
                                        app.ui_strings().launcher_open
                                    }
                                }))
                                .child(status_hint("Esc", {
                                    let app = app.clone();
                                    move || {
                                        let _ = observed_locale.get();
                                        app.ui_strings().launcher_close
                                    }
                                })),
                        )
                        .child(
                            text(move || result_position(selected.get(), rows.with(Vec::len)))
                                .text_size(px(11.0))
                                .text_color(rgb(
                                    steward_ui_components::palette::RESULT_LABEL_FOREGROUND,
                                )),
                        ),
                ),
        )
}

fn launcher_height(visible_rows: usize) -> f32 {
    LAUNCHER_HEADER_HEIGHT + 8.0 + visible_rows as f32 * ROW_HEIGHT + LAUNCHER_STATUS_BAR_HEIGHT
}

fn status_hint(key: &'static str, label: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .child(
            view()
                .h(px(18.0))
                .min_w(px(22.0))
                .px(px(5.0))
                .rounded(px(5.0))
                .bg(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.08))
                .items_center()
                .justify_center()
                .child(
                    text(key).text_size(px(10.0)).text_color(
                        rgb(steward_ui_components::palette::FOREGROUND)
                            .opacity(steward_ui_components::palette::PLACEHOLDER_OPACITY),
                    ),
                ),
        )
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND)),
        )
}

fn result_position(selected: usize, len: usize) -> String {
    if len == 0 {
        "0 / 0".into()
    } else {
        format!("{} / {len}", selected.min(len - 1) + 1)
    }
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
    let current = current.min(len - 1);
    let next = (current as isize + delta).clamp(0, len as isize - 1) as usize;
    (next != current).then_some(next)
}

/// Enter activates the selected row. The calculator row is always first, so
/// evaluate the current query directly when it is selected instead of
/// depending on an asynchronous search reply having updated `actions` yet.
fn submission_action(actions: &[RowAction], selected: usize, text: &str) -> Option<RowAction> {
    if selected == 0
        && actions
            .first()
            .is_none_or(|action| matches!(action, RowAction::CopyText(_)))
    {
        if let Some(value) = steward_core_engine::try_evaluate(text.trim()) {
            return Some(RowAction::CopyText(steward_core_engine::format_value(
                value,
            )));
        }
    }
    actions.get(selected).cloned()
}

fn add_input_focus(input: &nsh_core::ui::ElementRef) {
    set_timeout(Duration::from_millis(10), {
        let input = *input;
        move || input.focus()
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicU64, Arc};
    use std::time::Duration;

    use nsh_core::ui::{EngineContext, Root};
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;

    use super::{
        launcher_height, next_selection, result_position, submission_action, StewardLauncher,
    };
    use crate::{
        i18n::UiStrings,
        services::{AppServices, RowAction, UiSettings},
    };

    #[test]
    fn selection_stops_at_the_result_bounds() {
        assert_eq!(next_selection(0, 3, 1), Some(1));
        assert_eq!(next_selection(2, 3, 1), None);
        assert_eq!(next_selection(2, 3, -1), Some(1));
        assert_eq!(next_selection(0, 3, -1), None);
        assert_eq!(next_selection(9, 3, 1), None);
        assert_eq!(next_selection(9, 3, -1), Some(1));
        assert_eq!(next_selection(0, 0, 1), None);
    }

    #[test]
    fn launcher_height_includes_the_status_bar() {
        assert_eq!(launcher_height(0), 96.0);
        assert_eq!(launcher_height(8), 432.0);
    }

    #[test]
    fn status_bar_reports_the_clamped_result_position() {
        assert_eq!(result_position(0, 0), "0 / 0");
        assert_eq!(result_position(0, 3), "1 / 3");
        assert_eq!(result_position(2, 3), "3 / 3");
        assert_eq!(result_position(9, 3), "3 / 3");
    }

    #[test]
    fn enter_copies_the_current_calculation_result() {
        assert_eq!(
            submission_action(&[], 0, "8*8"),
            Some(RowAction::CopyText("64".into()))
        );
    }

    #[test]
    fn enter_still_activates_a_selected_non_calculator_row() {
        let calculator = RowAction::CopyText("64".into());
        let action = RowAction::OpenUrl("https://example.com".into());
        assert_eq!(
            submission_action(&[calculator, action.clone()], 1, "8*8"),
            Some(action)
        );
    }

    #[test]
    fn enter_dismisses_a_notice_instead_of_copying_a_calculation() {
        let actions = [RowAction::DismissNotice, RowAction::CopyText("64".into())];
        assert_eq!(
            submission_action(&actions, 0, "8*8"),
            Some(RowAction::DismissNotice)
        );
    }

    #[test]
    fn summoning_again_clears_the_launcher_query() {
        let placeholder = "Search apps, files, settings, or type a command...";
        let (request_tx, _request_rx) = crossbeam_channel::unbounded();
        let (command_tx, _command_rx) = crossbeam_channel::unbounded();
        let services = Arc::new(AppServices::new(
            request_tx,
            command_tx,
            UiSettings::new(
                "en".into(),
                UiStrings {
                    search_placeholder: placeholder.into(),
                    application: "Application".into(),
                    file_search_unavailable: "Windows Search is unavailable".into(),
                    file_search_repair: "Open Windows Search settings".into(),
                    settings: "Windows Settings".into(),
                    plugin: "Plugin".into(),
                    calculator: "Calculator".into(),
                    open_url: "Open URL".into(),
                    launcher_navigate: "Navigate".into(),
                    launcher_open: "Open".into(),
                    launcher_close: "Close".into(),
                },
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
            ),
            Arc::new(AtomicU64::new(0)),
        ));
        let config = EngineConfig {
            context: EngineContext::default().with(std::sync::Arc::new(services.launcher())),
            ..EngineConfig::new(Root::of::<StewardLauncher>())
        };
        let mut app = Headless::start(config).unwrap();
        app.wait_for(&format!("text:{placeholder}")).unwrap();
        app.wait_for("text:Navigate").unwrap();
        app.type_text(&format!("text:{placeholder}"), "8*8")
            .unwrap();
        app.wait_for("text:8*8").unwrap();

        services.request_launcher_reset();

        app.wait_for(&format!("text:{placeholder}")).unwrap();
        app.type_text(&format!("text:{placeholder}"), "2+2")
            .unwrap();
        app.wait_for("text:2+2").unwrap();

        services.request_launcher_focus();
        app.wait(Duration::from_millis(120)).unwrap();
        app.type_text("text:2+2", "3+3").unwrap();

        app.wait_for("text:2+23+3").unwrap();
    }
}
