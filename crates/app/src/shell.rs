//! Native NSH Rust launcher root. This surface never starts QuickJS.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::{menu, menu_item, MenuEvent};
use nsh_core::canvas::{LineCap, LineJoin};
use nsh_core::element::{input, StyleRefinement};
use nsh_core::keymap::When;
use nsh_core::prelude::*;
use steward_ui_components::{result_list, LauncherRow};

use crate::config::{
    LAUNCHER_HEADER_HEIGHT, LAUNCHER_INPUT_HEIGHT, LAUNCHER_INPUT_INSET, LAUNCHER_MARGIN,
    LAUNCHER_STATUS_BAR_HEIGHT,
};
use crate::launcher_model::{FeatureId, LauncherItem, LauncherSection};
use crate::services::{services, LauncherServices, RowAction, WindowCommand};

const MAX_VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 42.0;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(40);
const ACTION_MENU_MIN_HEIGHT: f32 = 380.0;
const STATUS_HINT_HEIGHT: f32 = 26.0;
/// Keep reasserting the field across the activation/IME hand-off instead of
/// relying on one focus command landing on the right frame.
const FOCUS_RETRIES: u8 = 5;

#[derive(Clone, Copy)]
enum SelectionMode {
    Reset,
    Preserve,
}

type LauncherFilter = Rc<dyn Fn(&str, SelectionMode)>;

#[component]
pub fn StewardLauncher() -> impl IntoElement {
    let app: Arc<LauncherServices> = services().expect("launcher services in EngineContext");
    let query = signal(String::new());
    let rows = signal(Vec::<LauncherRow>::new());
    let actions = signal(Vec::<RowAction>::new());
    let selected = signal(0_usize);
    let first_visible = signal(0_usize);
    let generation = signal(0_u64);
    let actions_open = signal(false);
    let action_search = signal(String::new());
    let action_selected = signal(None::<ActionMenuItemKind>);
    let input_ref = element_ref();
    let action_input_ref = element_ref();
    let list_ref = element_ref();

    let filter: LauncherFilter = {
        let app = app.clone();
        Rc::new(move |text: &str, selection_mode: SelectionMode| {
            let preserve_selection = matches!(selection_mode, SelectionMode::Preserve);
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
                            let mut items = reply.items;
                            if let Some(notice) = app.notice() {
                                let action = match notice.kind {
                                    steward_plugin_contract::PluginNoticeKind::Confirmation {
                                        id,
                                        confirm_label,
                                        ..
                                    } => RowAction::ConfirmNotice {
                                        plugin_id: notice.plugin_id.unwrap_or_default(),
                                        id,
                                        label: confirm_label,
                                    },
                                    _ => RowAction::DismissNotice,
                                };
                                items.insert(
                                    0,
                                    LauncherItem::from_row(
                                        FeatureId::Notice,
                                        LauncherSection::Favorites,
                                        LauncherRow {
                                            id: "launch-notice".into(),
                                            title: notice.title,
                                            subtitle: notice.message,
                                            icon: None,
                                            subtitle_placement: Default::default(),
                                        },
                                        action,
                                    ),
                                );
                            }
                            let rows_value =
                                items.iter().map(LauncherItem::to_row).collect::<Vec<_>>();
                            let actions_value = items
                                .iter()
                                .map(|item| item.action.clone())
                                .collect::<Vec<_>>();
                            let previous_selected = selected.get_untracked();
                            let previous_first_visible = first_visible.get_untracked();
                            let selected_id = if preserve_selection {
                                rows.with_untracked(|rows| {
                                    rows.get(previous_selected).map(|row| row.id.clone())
                                })
                            } else {
                                None
                            };
                            let preserved_index = selected_id
                                .as_ref()
                                .and_then(|id| rows_value.iter().position(|row| &row.id == id));
                            let height = launcher_height(rows_value.len().min(MAX_VISIBLE_ROWS));
                            if !preserve_selection || preserved_index.is_none() {
                                actions_open.set(false);
                                action_search.set(String::new());
                                action_selected.set(None);
                            }
                            rows.set(rows_value);
                            actions.set(actions_value);
                            let selected_index = if preserve_selection {
                                preserved_index.unwrap_or_else(|| {
                                    rows.with_untracked(|rows| {
                                        previous_selected.min(rows.len().saturating_sub(1))
                                    })
                                })
                            } else {
                                0
                            };
                            selected.set(selected_index);
                            if preserve_selection {
                                let first = previous_first_visible;
                                if selected_index < first {
                                    list_ref.scroll_to_index(
                                        selected_index as u32,
                                        percentage(0.0),
                                        px(0.0),
                                        false,
                                    );
                                } else if selected_index >= first + MAX_VISIBLE_ROWS {
                                    list_ref.scroll_to_index(
                                        selected_index as u32,
                                        percentage(1.0),
                                        px(0.0),
                                        false,
                                    );
                                }
                            } else {
                                first_visible.set(0);
                                list_ref.scroll_to_index(0, percentage(0.0), px(0.0), false);
                            }
                            app.send_window_command(WindowCommand::Resize { height });
                        }
                    }
                });
            });
        })
    };
    filter("", SelectionMode::Reset);

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
                filter(&query.get_untracked(), SelectionMode::Preserve);
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
                filter("", SelectionMode::Reset);
            }
            if focus_changed {
                observed_launcher_focus.set(focus_revision);
            }
            if launcher_scrim_alpha != scrim_alpha.get_untracked() {
                scrim_alpha.set(launcher_scrim_alpha);
            }
            if reset_changed || focus_changed {
                app.send_window_command(WindowCommand::FocusLauncher);
                input_ref.focus();
                focus_retries.set(FOCUS_RETRIES);
            } else if focus_retries.get_untracked() > 0 {
                focus_retries.update(|remaining| *remaining -= 1);
                app.send_window_command(WindowCommand::FocusLauncher);
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
    let activate_selected_action: Rc<dyn Fn() -> bool> = {
        let app = app.clone();
        Rc::new(move || {
            let selected_index = selected.get_untracked();
            let action = actions.with_untracked(|actions| actions.get(selected_index).cloned());
            let items = action_menu_items(&app.ui_strings(), action.as_ref());
            let selected = action_selected.get_untracked();
            let Some(item) = items
                .iter()
                .find(|item| Some(item.kind) == selected && item.enabled)
            else {
                return false;
            };
            let command = action_menu_command(item.kind, action.as_ref());
            if let Some(command) = command {
                app.send_window_command(command);
                true
            } else {
                false
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
    let set_actions_open: Rc<dyn Fn(bool)> = {
        let app = app.clone();
        Rc::new(move |open: bool| {
            actions_open.set(open);
            if open {
                action_search.set(String::new());
                action_selected.set(Some(ActionMenuItemKind::Primary));
                let action_input_ref = action_input_ref;
                set_timeout(Duration::from_millis(10), move || {
                    action_input_ref.focus();
                });
            } else {
                let input_ref = input_ref;
                set_timeout(Duration::from_millis(10), move || {
                    input_ref.focus();
                });
            }
            let visible_rows = rows.with_untracked(Vec::len).min(MAX_VISIBLE_ROWS);
            let height = if open {
                launcher_height_with_actions(visible_rows)
            } else {
                launcher_height(visible_rows)
            };
            app.send_window_command(WindowCommand::Resize { height });
        })
    };
    let select_action_relative: Rc<dyn Fn(isize)> = {
        let app = app.clone();
        Rc::new(move |delta: isize| {
            let selected_index = selected.get_untracked();
            let action = actions.with_untracked(|actions| actions.get(selected_index).cloned());
            let items = action_menu_items(&app.ui_strings(), action.as_ref());
            let matches = filtered_action_indices(&items, &action_search.get_untracked());
            if let Some(next) =
                next_action_selection(&items, &matches, action_selected.get_untracked(), delta)
            {
                action_selected.set(Some(next));
            }
        })
    };
    let key_handler: Rc<dyn Fn(&str)> = {
        let app = app.clone();
        let select_relative = select_relative.clone();
        let select_action_relative = select_action_relative.clone();
        let activate_selected_action = activate_selected_action.clone();
        let submit = submit.clone();
        let set_actions_open = set_actions_open.clone();
        Rc::new(move |key: &str| {
            crate::metrics::value("launcher_key", 1, serde_json::json!({ "key": key }));
            match key {
                "up" | "ArrowUp" => {
                    if actions_open.get_untracked() {
                        select_action_relative(-1);
                    } else {
                        select_relative(-1);
                    }
                }
                "down" | "ArrowDown" => {
                    if actions_open.get_untracked() {
                        select_action_relative(1);
                    } else {
                        select_relative(1);
                    }
                }
                "escape" | "Escape" => {
                    if actions_open.get_untracked() {
                        set_actions_open(false);
                    } else if app.notice().is_some_and(|notice| {
                        matches!(
                            notice.kind,
                            steward_plugin_contract::PluginNoticeKind::Confirmation { .. }
                        )
                    }) {
                        app.send_window_command(WindowCommand::Activate(RowAction::DismissNotice));
                    } else {
                        app.send_window_command(WindowCommand::Hide);
                    }
                }
                "enter" | "Enter" => {
                    if actions_open.get_untracked() {
                        if activate_selected_action() {
                            set_actions_open(false);
                        }
                    } else {
                        submit(&query.get_untracked());
                    }
                }
                "ctrl+k" | "control+k" if actions.with_untracked(Vec::len) > 0 => {
                    set_actions_open(!actions_open.get_untracked());
                }
                _ => {}
            }
        })
    };

    bind_launcher_key("up", true, key_handler.clone());
    bind_launcher_key("down", true, key_handler.clone());
    bind_launcher_key("enter", false, key_handler.clone());
    bind_launcher_key("ctrl+k", false, key_handler.clone());
    bind_launcher_key("escape", false, key_handler);

    add_input_focus(&input_ref);
    let has_rows = move || rows.with(|rows| !rows.is_empty());
    let current_action: Rc<dyn Fn() -> Option<RowAction>> =
        Rc::new(move || actions.with(|actions| actions.get(selected.get()).cloned()));
    let run_as_administrator_handler: Rc<dyn Fn(&str)> = {
        let app = app.clone();
        let current_action = current_action.clone();
        Rc::new(move |_| {
            let action = current_action();
            if let Some(command) =
                action_menu_command(ActionMenuItemKind::RunAsAdministrator, action.as_ref())
            {
                app.send_window_command(command);
            }
        })
    };
    bind_launcher_key("ctrl+shift+enter", false, run_as_administrator_handler);
    let current_row: Rc<dyn Fn() -> LauncherRow> = {
        Rc::new(move || rows.with(|rows| rows.get(selected.get()).cloned().unwrap_or_default()))
    };
    let primary_action_label: Rc<dyn Fn() -> String> = {
        let app = app.clone();
        let current_action = current_action.clone();
        Rc::new(move || {
            let _ = observed_locale.get();
            primary_action_label(current_action().as_ref(), &app.ui_strings())
        })
    };

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
                                    move |text: &str| {
                                        crate::metrics::value(
                                            "launcher_input_update",
                                            text.chars().count() as u64,
                                            serde_json::json!({}),
                                        );
                                        filter(text, SelectionMode::Reset);
                                    }
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
                        .border_color(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.04))
                        .window_drag(false)
                        .child(selection_status_icon(rows, selected))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(primary_action_status_hint(
                                    {
                                        let primary_action_label = primary_action_label.clone();
                                        move || primary_action_label()
                                    },
                                    {
                                        let activate = activate.clone();
                                        move || {
                                            activate(selected.with_untracked(|selected| *selected));
                                        }
                                    },
                                ))
                                .child({
                                    let app = app.clone();
                                    let set_actions_open = set_actions_open.clone();
                                    let current_action = current_action.clone();
                                    let current_row = current_row.clone();
                                    let on_open_change = set_actions_open.clone();
                                    menu()
                                        .open(move || Some(actions_open.get()))
                                        .placement(Placement::Top)
                                        .align(Align::End)
                                        .popup_style(actions_popup_style)
                                        .on_open_change(move |event: &MenuEvent| {
                                            if let MenuEvent::OpenChange(open, _) = event {
                                                if *open && actions.with_untracked(Vec::len) == 0 {
                                                    return;
                                                }
                                                on_open_change(*open);
                                            }
                                        })
                                        .trigger(action_status_hint({
                                            let app = app.clone();
                                            move || {
                                                let _ = observed_locale.get();
                                                app.ui_strings().launcher_actions
                                            }
                                        }))
                                        .child(action_menu_content(
                                            app,
                                            current_action,
                                            current_row,
                                            ActionMenuState {
                                                search: action_search,
                                                selected: action_selected,
                                                input_ref: action_input_ref,
                                                activate_selected: activate_selected_action,
                                                set_open: set_actions_open,
                                            },
                                        ))
                                }),
                        ),
                ),
        )
}

fn launcher_height(visible_rows: usize) -> f32 {
    LAUNCHER_HEADER_HEIGHT + 8.0 + visible_rows as f32 * ROW_HEIGHT + LAUNCHER_STATUS_BAR_HEIGHT
}

fn launcher_height_with_actions(visible_rows: usize) -> f32 {
    launcher_height(visible_rows).max(ACTION_MENU_MIN_HEIGHT)
}

fn selection_status_icon(
    rows: Signal<Vec<LauncherRow>>,
    selected: Signal<usize>,
) -> impl IntoElement {
    let icon = move || {
        rows.with(|rows| {
            rows.get(selected.get())
                .and_then(|row| row.icon.clone())
                .unwrap_or_default()
        })
    };
    let has_icon =
        move || rows.with(|rows| rows.get(selected.get()).is_some_and(|r| r.icon.is_some()));
    view()
        .w(px(18.0))
        .h(px(18.0))
        .items_center()
        .justify_center()
        .when(has_icon(), move |status| {
            status.child(image(icon).w(px(18.0)).h(px(18.0)).flex_shrink_0())
        })
        .when(!has_icon(), |status| status.child(generic_status_icon()))
}

fn generic_status_icon() -> impl IntoElement {
    let color = rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND);
    view()
        .w(px(16.0))
        .h(px(16.0))
        .flex_col()
        .justify_center()
        .gap_1()
        .child(view().w(px(14.0)).h(px(2.0)).rounded(px(1.0)).bg(color))
        .child(view().w(px(10.0)).h(px(2.0)).rounded(px(1.0)).bg(color))
}

fn actions_popup_style(style: StyleRefinement) -> StyleRefinement {
    style
        .w(px(350.0))
        .p(px(0.0))
        .rounded(px(10.0))
        .bg(rgb(0x363636))
        .border_1()
        .border_color(rgb(0x5e5e5e))
        .overflow_hidden()
        .shadow_lg()
}

fn primary_action_status_hint(
    label: impl Fn() -> String + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(STATUS_HINT_HEIGHT))
        .px(px(6.0))
        .rounded(px(6.0))
        .hover(|style| style.bg(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.06)))
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(11.0))
                .line_height(px(22.0))
                .text_color(rgb(steward_ui_components::palette::FOREGROUND))
                .number_of_lines(1),
        )
        .child(enter_status_key())
}

fn action_status_hint(label: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(STATUS_HINT_HEIGHT))
        .px(px(6.0))
        .rounded(px(6.0))
        .hover(|style| style.bg(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.06)))
        .child(
            text(label)
                .text_size(px(11.0))
                .line_height(px(22.0))
                .text_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND)),
        )
        .child(status_key("Ctrl"))
        .child(status_key("K"))
}

fn primary_action_label(action: Option<&RowAction>, strings: &crate::i18n::UiStrings) -> String {
    match action {
        Some(RowAction::LaunchApp(_) | RowAction::RunAsAdministrator(_)) => {
            strings.launcher_open_application.clone()
        }
        Some(RowAction::OpenFile(_)) => strings.launcher_open_file.clone(),
        Some(RowAction::CopyText(_)) => strings.launcher_copy.clone(),
        Some(RowAction::OpenUrl(_)) => strings.open_url.clone(),
        Some(RowAction::OpenSettings(_)) => strings.launcher_open_settings.clone(),
        Some(RowAction::DismissNotice) => strings.launcher_close.clone(),
        Some(RowAction::ConfirmNotice { label, .. }) => label.clone(),
        Some(RowAction::OpenPlugin { .. }) => strings.launcher_open_plugin.clone(),
        Some(RowAction::RevealInExplorer(_) | RowAction::ShowProperties(_)) | None => {
            strings.launcher_open.clone()
        }
    }
}

fn status_key(key: &'static str) -> impl IntoElement {
    status_keycap(
        text(key)
            .text_size(px(10.0))
            .line_height(px(18.0))
            .text_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND)),
    )
}

fn enter_status_key() -> impl IntoElement {
    status_keycap(
        view().w(px(12.0)).h(px(12.0)).child(
            canvas(move || {
                Painter::new()
                    .stroke_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND))
                    .line_width(1.35)
                    .line_cap(LineCap::Round)
                    .line_join(LineJoin::Round)
                    .stroke_svg("M9 3.5 L9 7.5 L3.5 7.5 M5.5 5.5 L3.5 7.5 L5.5 9.5")
            })
            .size_full(),
        ),
    )
}

fn status_keycap(content: impl IntoElement) -> impl IntoElement {
    view()
        .h(px(18.0))
        .min_w(px(22.0))
        .px(px(5.0))
        .rounded(px(5.0))
        .bg(rgb(steward_ui_components::palette::FOREGROUND).opacity(0.08))
        .items_center()
        .justify_center()
        .child(content)
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum ActionMenuItemKind {
    Primary,
    ApplicationWindow,
    RecentFiles,
    RunAsAdministrator,
    RunAsDifferentUser,
    ShowInFileExplorer,
}

#[derive(Clone, Debug, PartialEq)]
struct ActionMenuItem {
    kind: ActionMenuItemKind,
    label: String,
    icon: ActionPanelIcon,
    keys: &'static [&'static str],
    enter: bool,
    enabled: bool,
}

struct ActionMenuState {
    search: Signal<String>,
    selected: Signal<Option<ActionMenuItemKind>>,
    input_ref: nsh_core::element::ElementRef,
    activate_selected: Rc<dyn Fn() -> bool>,
    set_open: Rc<dyn Fn(bool)>,
}

struct ActionPanelRowOptions {
    keys: &'static [&'static str],
    enabled: bool,
    enter: bool,
    close_on_select: bool,
}

fn primary_action_icon(action: Option<&RowAction>) -> ActionPanelIcon {
    match action {
        Some(RowAction::LaunchApp(_) | RowAction::RunAsAdministrator(_)) => ActionPanelIcon::Play,
        Some(RowAction::OpenFile(_)) => ActionPanelIcon::File,
        Some(RowAction::CopyText(_)) => ActionPanelIcon::Copy,
        Some(RowAction::OpenUrl(_)) => ActionPanelIcon::Globe,
        Some(RowAction::OpenSettings(_)) => ActionPanelIcon::Settings,
        Some(RowAction::DismissNotice) => ActionPanelIcon::Close,
        Some(RowAction::ConfirmNotice { .. }) => ActionPanelIcon::Play,
        Some(RowAction::OpenPlugin { .. }) => ActionPanelIcon::Package,
        Some(RowAction::RevealInExplorer(_)) => ActionPanelIcon::FolderOpen,
        Some(RowAction::ShowProperties(_)) | None => ActionPanelIcon::Application,
    }
}

fn action_menu_items(
    strings: &crate::i18n::UiStrings,
    action: Option<&RowAction>,
) -> Vec<ActionMenuItem> {
    let has_path = action.and_then(row_path).is_some();
    let can_run_as_administrator = action.is_some_and(can_run_as_administrator);
    vec![
        ActionMenuItem {
            kind: ActionMenuItemKind::Primary,
            label: primary_action_label(action, strings),
            icon: primary_action_icon(action),
            keys: &[],
            enter: true,
            enabled: action.is_some(),
        },
        ActionMenuItem {
            kind: ActionMenuItemKind::ApplicationWindow,
            label: strings.launcher_action_application_window.clone(),
            icon: ActionPanelIcon::ApplicationWindow,
            keys: &["Ctrl", "N"],
            enter: false,
            enabled: false,
        },
        ActionMenuItem {
            kind: ActionMenuItemKind::RecentFiles,
            label: strings.launcher_action_recent_files.clone(),
            icon: ActionPanelIcon::History,
            keys: &["Ctrl", "O"],
            enter: false,
            enabled: false,
        },
        ActionMenuItem {
            kind: ActionMenuItemKind::RunAsAdministrator,
            label: strings.launcher_action_run_as_administrator.clone(),
            icon: ActionPanelIcon::Shield,
            keys: &["Ctrl", "Shift"],
            enter: true,
            enabled: can_run_as_administrator,
        },
        ActionMenuItem {
            kind: ActionMenuItemKind::RunAsDifferentUser,
            label: strings.launcher_action_run_as_different_user.clone(),
            icon: ActionPanelIcon::Users,
            keys: &["Alt", "Shift"],
            enter: true,
            enabled: false,
        },
        ActionMenuItem {
            kind: ActionMenuItemKind::ShowInFileExplorer,
            label: strings.launcher_action_show_in_file_explorer.clone(),
            icon: ActionPanelIcon::FolderOpen,
            keys: &[],
            enter: false,
            enabled: has_path,
        },
    ]
}

fn can_run_as_administrator(action: &RowAction) -> bool {
    matches!(action, RowAction::LaunchApp(_) | RowAction::OpenFile(_))
}

fn action_menu_command(
    kind: ActionMenuItemKind,
    action: Option<&RowAction>,
) -> Option<WindowCommand> {
    match kind {
        ActionMenuItemKind::Primary => action.cloned().map(WindowCommand::Activate),
        ActionMenuItemKind::RunAsAdministrator => action
            .filter(|action| can_run_as_administrator(action))
            .and_then(row_path)
            .cloned()
            .map(RowAction::RunAsAdministrator)
            .map(WindowCommand::Activate),
        ActionMenuItemKind::ShowInFileExplorer => action
            .and_then(row_path)
            .cloned()
            .map(RowAction::RevealInExplorer)
            .map(WindowCommand::Activate),
        ActionMenuItemKind::ApplicationWindow
        | ActionMenuItemKind::RecentFiles
        | ActionMenuItemKind::RunAsDifferentUser => None,
    }
}

fn filtered_action_indices(items: &[ActionMenuItem], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (query.is_empty() || item.label.to_lowercase().contains(&query)).then_some(index)
        })
        .collect()
}

fn next_action_selection(
    items: &[ActionMenuItem],
    matches: &[usize],
    current: Option<ActionMenuItemKind>,
    delta: isize,
) -> Option<ActionMenuItemKind> {
    let enabled: Vec<_> = matches
        .iter()
        .copied()
        .filter(|&index| items.get(index).is_some_and(|item| item.enabled))
        .collect();
    if enabled.is_empty() {
        return None;
    }
    let index = enabled
        .iter()
        .position(|&candidate| Some(items[candidate].kind) == current)
        .map_or_else(
            || {
                if delta < 0 {
                    enabled.len() - 1
                } else {
                    0
                }
            },
            |position| (position as isize + delta).rem_euclid(enabled.len() as isize) as usize,
        );
    Some(items[enabled[index]].kind)
}

fn action_menu_item_handler(
    kind: ActionMenuItemKind,
    app: Arc<LauncherServices>,
    current_action: Rc<dyn Fn() -> Option<RowAction>>,
) -> impl Fn() + 'static {
    move || {
        activate_action_menu_item(kind, &app, &current_action);
    }
}

fn activate_action_menu_item(
    kind: ActionMenuItemKind,
    app: &LauncherServices,
    current_action: &Rc<dyn Fn() -> Option<RowAction>>,
) -> bool {
    let action = current_action();
    if let Some(command) = action_menu_command(kind, action.as_ref()) {
        app.send_window_command(command);
        true
    } else {
        false
    }
}

fn action_menu_content(
    app: Arc<LauncherServices>,
    current_action: Rc<dyn Fn() -> Option<RowAction>>,
    current_row: Rc<dyn Fn() -> LauncherRow>,
    state: ActionMenuState,
) -> impl IntoElement {
    let ActionMenuState {
        search: action_search,
        selected: action_selected,
        input_ref: action_input_ref,
        activate_selected: activate_selected_action,
        set_open: set_actions_open,
    } = state;
    let strings = app.ui_strings();
    let search_placeholder = strings.launcher_action_search.clone();
    let no_results = strings.launcher_action_no_results.clone();
    let actions = view()
        .flex_1()
        .flex_col()
        .px(px(8.0))
        .pt(px(8.0))
        .child({
            let current_action_for_items = current_action.clone();
            let strings = strings.clone();
            each(
                move || {
                    let action = current_action_for_items();
                    let items = action_menu_items(&strings, action.as_ref());
                    filtered_action_indices(&items, &action_search.get())
                        .into_iter()
                        .map(|index| items[index].clone())
                        .collect::<Vec<ActionMenuItem>>()
                },
                |item| item.kind,
                {
                    let app = app.clone();
                    let current_action = current_action.clone();
                    move |item: nsh_core::element::ReadSignal<ActionMenuItem>,
                          _: nsh_core::element::ReadSignal<usize>| {
                        let item = item.get_untracked();
                        let kind = item.kind;
                        action_panel_row(
                            item.label,
                            action_panel_icon(item.icon),
                            ActionPanelRowOptions {
                                keys: item.keys,
                                enabled: item.enabled,
                                enter: item.enter,
                                close_on_select: item.enabled,
                            },
                            move || action_selected.get() == Some(kind),
                            action_menu_item_handler(kind, app.clone(), current_action.clone()),
                        )
                    }
                },
            )
        })
        .child({
            let current_action = current_action.clone();
            let strings = strings.clone();
            when(
                move || {
                    let action = current_action();
                    let items = action_menu_items(&strings, action.as_ref());
                    filtered_action_indices(&items, &action_search.get()).is_empty()
                },
                move || {
                    view()
                        .w_full()
                        .h(px(40.0))
                        .items_center()
                        .justify_center()
                        .child(text(no_results.clone()).text_size(px(12.0)).text_color(rgb(
                            steward_ui_components::palette::RESULT_LABEL_FOREGROUND,
                        )))
                },
            )
        });

    view()
        .w_full()
        .h(px(280.0))
        .flex_col()
        .bg(rgb(0x363636))
        .child(action_menu_header(current_row, strings.application.clone()))
        .child(actions)
        .child(
            view()
                .h(px(39.0))
                .w_full()
                .px(px(16.0))
                .flex_row()
                .items_center()
                .gap_2()
                .border_t(px(1.0))
                .border_color(rgb(0x4a4a4a))
                .child(action_panel_icon(ActionPanelIcon::Search))
                .child(
                    input()
                        .test_id("action-search")
                        .element_ref(&action_input_ref)
                        .flex_1()
                        .h(px(30.0))
                        .bg(rgba(0x00000000))
                        .value(move || action_search.get())
                        .placeholder(move || search_placeholder.clone())
                        .on_change({
                            let current_action = current_action.clone();
                            let strings = strings.clone();
                            move |text: &str| {
                                action_search.set(text.to_owned());
                                let action = current_action();
                                let items = action_menu_items(&strings, action.as_ref());
                                let matches = filtered_action_indices(&items, text);
                                action_selected
                                    .set(next_action_selection(&items, &matches, None, 1));
                            }
                        })
                        .on_submit({
                            let activate_selected_action = activate_selected_action.clone();
                            let set_actions_open = set_actions_open.clone();
                            move |_text: &str| {
                                if activate_selected_action() {
                                    set_actions_open(false);
                                }
                            }
                        })
                        .text_size(px(12.0))
                        .text_color(rgb(0xffffff))
                        .placeholder_color(rgb(
                            steward_ui_components::palette::RESULT_LABEL_FOREGROUND,
                        ))
                        .selection_color(rgb(steward_ui_components::palette::ACCENT)),
                ),
        )
}

fn action_panel_row(
    label: impl Into<String>,
    icon: impl IntoElement,
    options: ActionPanelRowOptions,
    selected: impl Fn() -> bool + 'static,
    on_select: impl Fn() + 'static,
) -> impl IntoElement {
    menu_item()
        .disabled(!options.enabled)
        .on_select(on_select)
        .close_on_select(options.close_on_select)
        .w_full()
        .h(px(32.0))
        .px(px(8.0))
        .rounded(px(8.0))
        .flex_row()
        .items_center()
        .gap_2()
        .bg(move || {
            if selected() {
                rgb(0x4a4a4a)
            } else {
                rgba(0x00000000)
            }
        })
        .hover(|style| style.bg(rgb(0x4a4a4a)))
        .child(icon)
        .child(
            view().flex_1().min_w(px(0.0)).child(
                text(label.into())
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(rgb(0xffffff))
                    .number_of_lines(1),
            ),
        )
        .child(action_shortcut_hint(options.keys, options.enter))
}

fn action_shortcut_hint(keys: &'static [&'static str], enter: bool) -> impl IntoElement {
    let mut hint = view().flex_row().items_center().gap_1();
    for key in keys {
        hint = hint.child(action_shortcut_key(key));
    }
    if enter {
        hint = hint.child(action_shortcut_enter());
    }
    hint
}

fn action_shortcut_key(key: &'static str) -> impl IntoElement {
    action_shortcut_keycap(
        text(key)
            .text_size(px(10.0))
            .line_height(px(18.0))
            .text_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND)),
    )
}

fn action_shortcut_enter() -> impl IntoElement {
    action_shortcut_keycap(
        view().w(px(12.0)).h(px(12.0)).child(
            canvas(move || {
                Painter::new()
                    .stroke_color(rgb(steward_ui_components::palette::RESULT_LABEL_FOREGROUND))
                    .line_width(1.35)
                    .line_cap(LineCap::Round)
                    .line_join(LineJoin::Round)
                    .stroke_svg("M9 3.5 L9 7.5 L3.5 7.5 M5.5 5.5 L3.5 7.5 L5.5 9.5")
            })
            .size_full(),
        ),
    )
}

fn action_shortcut_keycap(content: impl IntoElement) -> impl IntoElement {
    view()
        .h(px(18.0))
        .min_w(px(22.0))
        .px(px(5.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(rgb(0x5e5e5e))
        .items_center()
        .justify_center()
        .child(content)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionPanelIcon {
    Application,
    ApplicationWindow,
    Play,
    File,
    Copy,
    Globe,
    Settings,
    Close,
    Package,
    History,
    Shield,
    Users,
    FolderOpen,
    Search,
}

fn action_menu_header(
    current_row: Rc<dyn Fn() -> LauncherRow>,
    fallback_title: String,
) -> impl IntoElement {
    let has_icon_row = current_row.clone();
    let icon_row = current_row.clone();
    let title_row = current_row;
    let has_icon = move || has_icon_row().icon.is_some();
    let icon_source: Rc<dyn Fn() -> String> = Rc::new(move || icon_row().icon.unwrap_or_default());
    view()
        .h(px(40.0))
        .w_full()
        .px(px(16.0))
        .flex_row()
        .items_center()
        .gap_2()
        .border_b(px(1.0))
        .border_color(rgb(0x4a4a4a))
        .child(
            view()
                .w(px(18.0))
                .h(px(18.0))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(
                    when(has_icon, move || {
                        let icon_source = icon_source.clone();
                        image(move || icon_source()).w(px(18.0)).h(px(18.0))
                    })
                    .otherwise(|| action_panel_icon_sized(ActionPanelIcon::Application, 18.0)),
                ),
        )
        .child(
            view().flex_1().min_w(px(0.0)).child(
                text(move || {
                    let row = title_row();
                    if row.title.is_empty() {
                        fallback_title.clone()
                    } else {
                        row.title
                    }
                })
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(rgb(0xffffff))
                .number_of_lines(1),
            ),
        )
}

fn action_panel_icon(kind: ActionPanelIcon) -> impl IntoElement {
    action_panel_icon_sized(kind, 14.0)
}

fn action_panel_icon_sized(kind: ActionPanelIcon, size: f32) -> impl IntoElement {
    view().w(px(size)).h(px(size)).child(
        canvas(move || {
            // Lucide icons (ISC), kept at their 24-unit viewBox and scaled to the row size.
            Painter::new()
                .scale(size / 24.0, size / 24.0)
                .stroke_color(rgb(0xd0d0d0))
                .line_width(2.0)
                .line_cap(LineCap::Round)
                .line_join(LineJoin::Round)
                .stroke_svg(match kind {
                    ActionPanelIcon::Application => {
                        "M4 4 H20 A2 2 0 0 1 22 6 V18 A2 2 0 0 1 20 20 H4 A2 2 0 0 1 2 18 V6 A2 2 0 0 1 4 4 Z M10 4 V8 M2 8 H22 M6 4 V8"
                    }
                    ActionPanelIcon::ApplicationWindow => {
                        "M5 3 H19 A2 2 0 0 1 21 5 V19 A2 2 0 0 1 19 21 H5 A2 2 0 0 1 3 19 V5 A2 2 0 0 1 5 3 Z M3 9 H21"
                    }
                    ActionPanelIcon::Play => "M6 3 L20 12 L6 21 Z",
                    ActionPanelIcon::File => {
                        "M15 2 H6 A2 2 0 0 0 4 4 V20 A2 2 0 0 0 6 22 H18 A2 2 0 0 0 20 20 V7 Z M14 2 V6 A2 2 0 0 0 16 8 H20"
                    }
                    ActionPanelIcon::Copy => {
                        "M8 8 H18 A2 2 0 0 1 20 10 V20 A2 2 0 0 1 18 22 H8 A2 2 0 0 1 6 20 V10 A2 2 0 0 1 8 8 Z M4 16 C2.9 16 2 15.1 2 14 V4 C2 2.9 2.9 2 4 2 H14 C15.1 2 16 2.9 16 4"
                    }
                    ActionPanelIcon::Globe => {
                        "M12 2 A10 10 0 1 0 12 22 A10 10 0 1 0 12 2 Z M12 2 A14.5 14.5 0 0 0 12 22 M12 2 A14.5 14.5 0 0 1 12 22 M2 12 H22"
                    }
                    ActionPanelIcon::Settings => {
                        "M21 4 H14 M10 4 H3 M21 12 H12 M8 12 H3 M21 20 H16 M12 20 H3 M14 2 V6 M8 10 V14 M16 18 V22"
                    }
                    ActionPanelIcon::Close => "M18 6 L6 18 M6 6 L18 18",
                    ActionPanelIcon::Package => {
                        "M21 8 A2 2 0 0 0 20 6.27 L13 2.27 A2 2 0 0 0 11 2.27 L4 6.27 A2 2 0 0 0 3 8 V16 A2 2 0 0 0 4 17.73 L11 21.73 A2 2 0 0 0 13 21.73 L20 17.73 A2 2 0 0 0 21 16 Z M3.3 7 L12 12 L20.7 7 M12 22 V12"
                    }
                    ActionPanelIcon::History => {
                        "M3 12 A9 9 0 1 0 12 3 A9.75 9.75 0 0 0 5.26 5.74 L3 8 M3 3 V8 H8 M12 7 V12 L16 14"
                    }
                    ActionPanelIcon::Shield => {
                        "M20 13 C20 18 16.5 20.5 12.34 21.95 A1 1 0 0 1 11.67 21.94 C7.5 20.5 4 18 4 13 V6 A1 1 0 0 1 5 5 C7 5 9.5 3.8 11.24 2.28 A1.17 1.17 0 0 1 12.76 2.28 C14.51 3.81 17 5 19 5 A1 1 0 0 1 20 6 Z M9 12 L11 14 L15 10"
                    }
                    ActionPanelIcon::Users => {
                        "M18 21 A8 8 0 0 0 2 21 M10 3 A5 5 0 1 0 10 13 A5 5 0 1 0 10 3 Z M22 20 C22 16.63 20 13.5 18 12 A5 5 0 0 0 17.55 3.7"
                    }
                    ActionPanelIcon::FolderOpen => {
                        "M6 14 L7.5 11.1 A2 2 0 0 1 9.24 10 H20 A2 2 0 0 1 21.94 12.5 L20.4 18.5 A2 2 0 0 1 18.45 20 H4 A2 2 0 0 1 2 18 V5 A2 2 0 0 1 4 3 H7.9 A2 2 0 0 1 9.59 3.9 L10.4 5.1 A2 2 0 0 0 12.07 6 H18 A2 2 0 0 1 20 8 V10"
                    }
                    ActionPanelIcon::Search => {
                        "M11 3 A8 8 0 1 0 11 19 A8 8 0 1 0 11 3 Z M21 21 L16.7 16.7"
                    }
                })
        })
        .size_full(),
    )
}

fn row_path(action: &RowAction) -> Option<&std::path::PathBuf> {
    match action {
        RowAction::LaunchApp(path)
        | RowAction::RunAsAdministrator(path)
        | RowAction::OpenFile(path) => Some(path),
        _ => None,
    }
}

fn bind_launcher_key(key: &'static str, repeat: bool, handler: Rc<dyn Fn(&str)>) {
    on_key_with(
        key,
        KeyOptions {
            when: When::Always,
            repeat,
            pass_through: false,
            for_focus: false,
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

fn add_input_focus(input: &nsh_core::element::ElementRef) {
    set_timeout(Duration::from_millis(10), {
        let input = *input;
        move || input.focus()
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;

    use nsh_core::element::EngineContext;
    use nsh_engine::EngineConfig;
    use nsh_engine::Root;
    use nsh_testing::Headless;
    use steward_ui_components::LauncherRow;

    use super::{
        action_menu_command, action_menu_items, filtered_action_indices, launcher_height,
        launcher_height_with_actions, next_action_selection, next_selection, primary_action_label,
        row_path, submission_action, ActionMenuItemKind, StewardLauncher,
    };
    use crate::{
        i18n::UiStrings,
        launcher_model::{FeatureId, LauncherItem, LauncherSection},
        services::{AppServices, RowAction, SearchReply, SearchRequest, UiSettings, WindowCommand},
    };

    fn items_from_rows(rows: Vec<LauncherRow>, actions: Vec<RowAction>) -> Vec<LauncherItem> {
        assert_eq!(rows.len(), actions.len());
        rows.into_iter()
            .zip(actions)
            .map(|(row, action)| {
                LauncherItem::from_row(
                    FeatureId::Application,
                    LauncherSection::Applications,
                    row,
                    action,
                )
            })
            .collect()
    }

    fn ui_strings(placeholder: &str) -> UiStrings {
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
            launcher_confirm: "Confirm".into(),
            launcher_actions: "Actions".into(),
            launcher_copy: "Copy".into(),
            launcher_open_application: "Open Application".into(),
            launcher_open_file: "Open File".into(),
            launcher_open_settings: "Open Settings".into(),
            launcher_open_plugin: "Open Plugin".into(),
            launcher_open_location: "Open file location".into(),
            launcher_copy_path: "Copy path".into(),
            launcher_properties: "Properties".into(),
            launcher_action_application_window: "Application Window".into(),
            launcher_action_recent_files: "Open Files... / Recent".into(),
            launcher_action_run_as_administrator: "Run as Administrator".into(),
            launcher_action_run_as_different_user: "Run as Different User".into(),
            launcher_action_show_in_file_explorer: "Show in File Explorer".into(),
            launcher_action_search: "Search for actions...".into(),
            launcher_action_no_results: "No matching actions".into(),
        }
    }

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
        assert_eq!(launcher_height(0), 102.0);
        assert_eq!(launcher_height(8), 438.0);
        assert_eq!(launcher_height_with_actions(0), 380.0);
        assert_eq!(launcher_height_with_actions(8), 438.0);
    }

    #[test]
    fn file_result_actions_expose_the_path_for_context_commands() {
        let path = PathBuf::from(r"C:\Windows\notepad.exe");
        assert_eq!(row_path(&RowAction::LaunchApp(path.clone())), Some(&path));
        assert_eq!(row_path(&RowAction::OpenFile(path.clone())), Some(&path));
        assert_eq!(row_path(&RowAction::CopyText("text".into())), None);
    }

    #[test]
    fn application_results_show_the_open_application_status_action() {
        let strings = ui_strings("Search");
        let action = RowAction::LaunchApp(PathBuf::from(r"C:\Windows\notepad.exe"));
        assert_eq!(
            primary_action_label(Some(&action), &strings),
            "Open Application"
        );
    }

    #[test]
    fn action_menu_search_filters_labels_without_case_sensitivity() {
        let strings = ui_strings("Search");
        let action = RowAction::LaunchApp(PathBuf::from(r"C:\Windows\notepad.exe"));
        let items = action_menu_items(&strings, Some(&action));

        let matches = filtered_action_indices(&items, "ADMINISTRATOR");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            items[matches[0]].kind,
            ActionMenuItemKind::RunAsAdministrator
        );

        let matches = filtered_action_indices(&items, "file explorer");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            items[matches[0]].kind,
            ActionMenuItemKind::ShowInFileExplorer
        );
        assert!(filtered_action_indices(&items, "missing action").is_empty());
    }

    #[test]
    fn action_menu_keyboard_navigation_skips_unavailable_actions() {
        let strings = ui_strings("Search");
        let action = RowAction::LaunchApp(PathBuf::from(r"C:\Windows\notepad.exe"));
        let items = action_menu_items(&strings, Some(&action));
        let all = filtered_action_indices(&items, "");

        assert_eq!(
            next_action_selection(&items, &all, Some(ActionMenuItemKind::Primary), 1),
            Some(ActionMenuItemKind::RunAsAdministrator)
        );
        assert_eq!(
            next_action_selection(
                &items,
                &all,
                Some(ActionMenuItemKind::RunAsAdministrator),
                1
            ),
            Some(ActionMenuItemKind::ShowInFileExplorer)
        );
        assert_eq!(
            next_action_selection(
                &items,
                &all,
                Some(ActionMenuItemKind::ShowInFileExplorer),
                1
            ),
            Some(ActionMenuItemKind::Primary)
        );

        let administrator = filtered_action_indices(&items, "administrator");
        assert_eq!(
            next_action_selection(&items, &administrator, None, 1),
            Some(ActionMenuItemKind::RunAsAdministrator)
        );
        let application_window = filtered_action_indices(&items, "application window");
        assert_eq!(
            next_action_selection(&items, &application_window, None, 1),
            None
        );
    }

    #[test]
    fn administrator_action_is_available_for_paths_but_not_other_results() {
        let strings = ui_strings("Search");
        let path = PathBuf::from(r"C:\Windows\notepad.exe");
        let app = RowAction::LaunchApp(path.clone());
        let file = RowAction::OpenFile(path.clone());
        let url = RowAction::OpenUrl("https://example.com".into());

        let enabled = |action: &RowAction| {
            action_menu_items(&strings, Some(action))
                .into_iter()
                .find(|item| item.kind == ActionMenuItemKind::RunAsAdministrator)
                .is_some_and(|item| item.enabled)
        };
        assert!(enabled(&app));
        assert!(enabled(&file));
        assert!(!enabled(&url));
        assert!(matches!(
            action_menu_command(ActionMenuItemKind::RunAsAdministrator, Some(&app)),
            Some(WindowCommand::Activate(RowAction::RunAsAdministrator(target)))
                if target == path
        ));
        assert!(matches!(
            action_menu_command(ActionMenuItemKind::ShowInFileExplorer, Some(&app)),
            Some(WindowCommand::Activate(RowAction::RevealInExplorer(target)))
                if target == path
        ));
    }

    #[test]
    fn action_menu_search_is_editable_and_keyboard_selection_executes() {
        let placeholder = "Search apps, files, settings, or type a command...";
        let app_path = PathBuf::from(r"C:\Windows\notepad.exe");
        let reply_path = app_path.clone();
        let (request_tx, request_rx) = crossbeam_channel::unbounded::<SearchRequest>();
        std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let _ = request.reply.send(SearchReply {
                    generation: request.generation,
                    items: items_from_rows(
                        vec![LauncherRow {
                            id: "app:notepad".into(),
                            title: "Notepad".into(),
                            subtitle: "Application".into(),
                            icon: None,
                            subtitle_placement: Default::default(),
                        }],
                        vec![RowAction::LaunchApp(reply_path.clone())],
                    ),
                });
            }
        });
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let services = Arc::new(AppServices::new(
            request_tx,
            command_tx,
            UiSettings::new(
                "en".into(),
                ui_strings(placeholder),
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
                false,
            ),
            Arc::new(AtomicU64::new(0)),
        ));
        let config = EngineConfig {
            context: EngineContext::default().with(std::sync::Arc::new(services.launcher())),
            ..EngineConfig::new(Root::of::<StewardLauncher>())
        };
        let mut app = Headless::start(config).unwrap();

        app.wait_for("text:Notepad").unwrap();
        app.press_key("ctrl+shift+enter").unwrap();
        let shortcut_commands = command_rx.try_iter().collect::<Vec<_>>();
        assert!(
            shortcut_commands.iter().any(|command| matches!(
                command,
                WindowCommand::Activate(RowAction::RunAsAdministrator(path)) if path == &app_path
            )),
            "{shortcut_commands:?}"
        );

        app.press("text:Actions").unwrap();
        app.wait_for("text:Search for actions...").unwrap();
        app.wait_for("text:Application Window").unwrap();

        app.type_text("id:action-search", "administrator").unwrap();
        app.wait_for("text:administrator").unwrap();
        app.wait_for("text:Run as Administrator").unwrap();
        app.wait_for_gone("text:Application Window", Duration::from_millis(500))
            .unwrap();
        app.press_key("enter").unwrap();

        let commands = command_rx.try_iter().collect::<Vec<_>>();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                WindowCommand::Activate(RowAction::RunAsAdministrator(path)) if path == &app_path
            )),
            "{commands:?}"
        );

        app.wait_for_gone("id:action-search", Duration::from_millis(500))
            .unwrap();
        app.press("text:Actions").unwrap();
        app.wait_for("id:action-search").unwrap();
        app.type_text("id:action-search", "zzzz").unwrap();
        app.wait_for("text:No matching actions").unwrap();
    }

    #[test]
    fn action_menu_header_and_actions_follow_the_selected_result() {
        let placeholder = "Search apps, files, settings, or type a command...";
        let notepad_path = PathBuf::from(r"C:\Windows\notepad.exe");
        let zed_path = PathBuf::from(r"C:\Program Files\Zed\zed.exe");
        let reply_notepad = notepad_path.clone();
        let reply_zed = zed_path.clone();
        let (request_tx, request_rx) = crossbeam_channel::unbounded::<SearchRequest>();
        std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let _ = request.reply.send(SearchReply {
                    generation: request.generation,
                    items: items_from_rows(
                        vec![
                            LauncherRow {
                                id: "app:notepad".into(),
                                title: "Notepad".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                            LauncherRow {
                                id: "app:zed".into(),
                                title: "Zed".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                        ],
                        vec![
                            RowAction::LaunchApp(reply_notepad.clone()),
                            RowAction::LaunchApp(reply_zed.clone()),
                        ],
                    ),
                });
            }
        });
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let services = Arc::new(AppServices::new(
            request_tx,
            command_tx,
            UiSettings::new(
                "en".into(),
                ui_strings(placeholder),
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
                false,
            ),
            Arc::new(AtomicU64::new(0)),
        ));
        let config = EngineConfig {
            context: EngineContext::default().with(std::sync::Arc::new(services.launcher())),
            ..EngineConfig::new(Root::of::<StewardLauncher>())
        };
        let mut app = Headless::start(config).unwrap();

        app.wait_for("text:Zed").unwrap();
        app.press_key("down").unwrap();
        app.press("text:Actions").unwrap();
        app.wait_for("id:action-search").unwrap();

        assert!(
            app.screen()
                .texts()
                .iter()
                .filter(|text| **text == "Zed")
                .count()
                >= 2,
            "the popup header should repeat the selected result name: {:?}",
            app.screen().texts()
        );

        app.press_key("enter").unwrap();
        assert!(command_rx.try_iter().any(|command| matches!(
            command,
            WindowCommand::Activate(RowAction::LaunchApp(path)) if path == zed_path
        )));
    }

    #[test]
    fn background_refresh_preserves_selection_and_action_menu() {
        let placeholder = "Search apps, files, settings, or type a command...";
        let notepad_path = PathBuf::from(r"C:\Windows\notepad.exe");
        let zed_path = PathBuf::from(r"C:\Program Files\Zed\zed.exe");
        let aardvark_path = PathBuf::from(r"C:\Program Files\Aardvark\aardvark.exe");
        let reply_notepad = notepad_path.clone();
        let reply_zed = zed_path.clone();
        let reply_aardvark = aardvark_path.clone();
        let (request_tx, request_rx) = crossbeam_channel::unbounded::<SearchRequest>();
        let request_count = Arc::new(AtomicUsize::new(0));
        let reply_count = request_count.clone();
        std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let response_number = reply_count.fetch_add(1, Ordering::Relaxed) + 1;
                let (rows, actions) = if response_number == 1 {
                    (
                        vec![
                            LauncherRow {
                                id: "app:notepad".into(),
                                title: "Notepad".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                            LauncherRow {
                                id: "app:zed".into(),
                                title: "Zed".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                        ],
                        vec![
                            RowAction::LaunchApp(reply_notepad.clone()),
                            RowAction::LaunchApp(reply_zed.clone()),
                        ],
                    )
                } else {
                    (
                        vec![
                            LauncherRow {
                                id: "app:aardvark".into(),
                                title: "Aardvark".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                            LauncherRow {
                                id: "app:notepad".into(),
                                title: "Notepad".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                            LauncherRow {
                                id: "app:zed".into(),
                                title: "Zed".into(),
                                subtitle: "Application".into(),
                                icon: None,
                                subtitle_placement: Default::default(),
                            },
                        ],
                        vec![
                            RowAction::LaunchApp(reply_aardvark.clone()),
                            RowAction::LaunchApp(reply_notepad.clone()),
                            RowAction::LaunchApp(reply_zed.clone()),
                        ],
                    )
                };
                let _ = request.reply.send(SearchReply {
                    generation: request.generation,
                    items: items_from_rows(rows, actions),
                });
            }
        });
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let search_revision = Arc::new(AtomicU64::new(0));
        let services = Arc::new(AppServices::new(
            request_tx,
            command_tx,
            UiSettings::new(
                "en".into(),
                ui_strings(placeholder),
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
                false,
            ),
            search_revision.clone(),
        ));
        let config = EngineConfig {
            context: EngineContext::default().with(std::sync::Arc::new(services.launcher())),
            ..EngineConfig::new(Root::of::<StewardLauncher>())
        };
        let mut app = Headless::start(config).unwrap();

        app.wait_for("text:Zed").unwrap();
        app.press_key("down").unwrap();
        app.press_key("ctrl+k").unwrap();
        app.wait_for("id:action-search").unwrap();
        let initial_requests = request_count.load(Ordering::Relaxed);

        search_revision.fetch_add(1, Ordering::Relaxed);
        for _ in 0..40 {
            if request_count.load(Ordering::Relaxed) > initial_requests {
                break;
            }
            app.wait(Duration::from_millis(25)).unwrap();
        }
        assert!(
            request_count.load(Ordering::Relaxed) > initial_requests,
            "the revision change should trigger a background search"
        );
        app.wait(Duration::from_millis(50)).unwrap();

        app.wait_for("id:action-search").unwrap();
        assert!(
            app.screen()
                .texts()
                .iter()
                .filter(|text| **text == "Zed")
                .count()
                >= 2,
            "the selected row should remain Zed after the refreshed list inserts Aardvark"
        );

        app.press_key("enter").unwrap();
        assert!(command_rx.try_iter().any(|command| matches!(
            command,
            WindowCommand::Activate(RowAction::LaunchApp(path)) if path == zed_path
        )));
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
                ui_strings(placeholder),
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
                false,
            ),
            Arc::new(AtomicU64::new(0)),
        ));
        let config = EngineConfig {
            context: EngineContext::default().with(std::sync::Arc::new(services.launcher())),
            ..EngineConfig::new(Root::of::<StewardLauncher>())
        };
        let mut app = Headless::start(config).unwrap();
        app.wait_for(&format!("text:{placeholder}")).unwrap();
        app.wait_for("text:Actions").unwrap();
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
