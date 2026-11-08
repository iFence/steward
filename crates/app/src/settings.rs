//! Native NSH settings root.

use std::rc::Rc;
use std::sync::Arc;

use nsh_core::ui::{prelude::*, px, rgb};

use crate::services::{services, AppServices, WindowCommand};

const LANGUAGES: [&str; 7] = ["en", "zh", "ja", "ko", "fr", "de", "ru"];
const ACCENTS: [(&str, u32); 5] = [
    ("#863bff", 0x863bff),
    ("#4f8cff", 0x4f8cff),
    ("#2fbf71", 0x2fbf71),
    ("#ff8a3d", 0xff8a3d),
    ("#e85d75", 0xe85d75),
];

#[component]
pub fn StewardSettings() -> impl IntoElement {
    let app: Arc<AppServices> = services().expect("settings AppServices in EngineContext");
    let language = signal(app.locale());
    let accent = signal(app.accent());
    let accent_draft = signal(app.accent());
    let accent_status = signal(String::new());
    let autostart = signal(app.autostart());
    let hotkey = signal(app.summon_hotkey());
    let hotkey_status = signal(String::new());

    let select_language = {
        let app = app.clone();
        move |code: &'static str| {
            language.set(code.to_owned());
            app.send_window_command(WindowCommand::SetLanguage(code.to_owned()));
        }
    };
    let select_accent = {
        let app = app.clone();
        move |value: &'static str| {
            accent.set(value.to_owned());
            accent_draft.set(value.to_owned());
            app.send_window_command(WindowCommand::SetAccent(value.to_owned()));
        }
    };
    let commit_accent = {
        let app = app.clone();
        Rc::new(move |value: &str| {
            let value = value.trim().to_owned();
            if parse_hex_color(&value).is_none() {
                accent_status.set("Use a six-digit hex color, for example #863bff.".into());
                return;
            }
            accent.set(value.clone());
            accent_draft.set(value.clone());
            accent_status.set("Saved.".into());
            app.send_window_command(WindowCommand::SetAccent(value));
        })
    };
    let commit_hotkey = {
        let app = app.clone();
        Rc::new(move |value: &str| {
            let value = value.trim().to_owned();
            if value.is_empty() {
                hotkey_status.set("Enter a key binding.".into());
                return;
            }
            hotkey.set(value.clone());
            hotkey_status.set("Applying binding...".into());
            app.send_window_command(WindowCommand::SetSummonHotkey(value));
        })
    };

    let accent_color =
        move || parse_hex_color(&accent.get()).unwrap_or(steward_ui_components::palette::ACCENT);

    view()
        .size_full()
        .p_6()
        .gap_5()
        .bg(rgb(steward_ui_components::palette::BACKGROUND))
        .child(
            text("Settings")
                .text_size(px(26.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
        )
        .child(section(
            "General",
            view()
                .gap_3()
                .child(
                    text("Language")
                        .text_size(px(14.0))
                        .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .children(LANGUAGES.map(|code| {
                            let select_language = select_language.clone();
                            nsh_base::button()
                                .selected(move || language.get() == code)
                                .on_press(move || select_language(code))
                                .px_3()
                                .py_2()
                                .rounded(px(7.0))
                                .bg(move || {
                                    if language.get() == code {
                                        rgb(accent_color())
                                    } else {
                                        rgb(steward_ui_components::palette::BACKGROUND_ALT)
                                    }
                                })
                                .child(
                                    text(code).text_size(px(13.0)).text_color(rgb(
                                        steward_ui_components::palette::FOREGROUND,
                                    )),
                                )
                                .into_element()
                        })),
                )
                .child(
                    nsh_base::switch()
                        .checked(autostart)
                        .on_change({
                            let app = app.clone();
                            move |enabled: &bool| {
                                autostart.set(*enabled);
                                app.send_window_command(WindowCommand::SetAutostart(*enabled));
                            }
                        })
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .track_style(move |style| {
                            style
                                .w(px(42.0))
                                .h(px(24.0))
                                .p(px(3.0))
                                .rounded_full()
                                .bg(rgb(steward_ui_components::palette::BORDER))
                                .checked(move |style| style.bg(rgb(accent_color())))
                        })
                        .thumb_style(|style| {
                            style
                                .size(px(18.0))
                                .rounded_full()
                                .bg(rgb(steward_ui_components::palette::FOREGROUND))
                                .checked(|style| style.translate_x(px(18.0)))
                        })
                        .child(
                            text("Launch at startup")
                                .text_size(px(14.0))
                                .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                        ),
                ),
        ))
        .child(section(
            "Appearance",
            view()
                .gap_3()
                .child(
                    text(move || format!("Accent: {}", accent.get()))
                        .text_size(px(14.0))
                        .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                )
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .children(ACCENTS.map(|(value, color)| {
                            let select_accent = select_accent.clone();
                            nsh_base::button()
                                .accessibility_label(format!("Use accent {value}"))
                                .selected(move || accent.get() == value)
                                .on_press(move || select_accent(value))
                                .size(px(34.0))
                                .rounded_full()
                                .bg(rgb(color))
                                .border_2()
                                .border_color(move || {
                                    if accent.get() == value {
                                        rgb(steward_ui_components::palette::FOREGROUND)
                                    } else {
                                        rgb(0x00000000)
                                    }
                                })
                                .into_element()
                        })),
                )
                .child(
                    input()
                        .value(move || accent_draft.get())
                        .placeholder("#863bff")
                        .on_change(move |value: &str| accent_draft.set(value.to_owned()))
                        .on_submit({
                            let commit_accent = commit_accent.clone();
                            move |value: &str| commit_accent(value)
                        })
                        .h(px(38.0))
                        .px_3()
                        .rounded(px(7.0))
                        .bg(rgb(steward_ui_components::palette::BACKGROUND_ALT))
                        .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                )
                .child(
                    text(move || accent_status.get())
                        .text_size(px(12.0))
                        .text_color(rgb(steward_ui_components::palette::MUTED_FOREGROUND)),
                ),
        ))
        .child(section(
            "Hotkeys",
            view()
                .gap_3()
                .child(
                    text("Summon Steward")
                        .text_size(px(14.0))
                        .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                )
                .child(
                    input()
                        .value(move || hotkey.get())
                        .placeholder("control+alt+Space")
                        .on_change(move |value: &str| hotkey.set(value.to_owned()))
                        .on_submit({
                            let commit_hotkey = commit_hotkey.clone();
                            move |value: &str| commit_hotkey(value)
                        })
                        .h(px(38.0))
                        .px_3()
                        .rounded(px(7.0))
                        .bg(rgb(steward_ui_components::palette::BACKGROUND_ALT))
                        .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
                )
                .child(
                    text(move || {
                        if hotkey_status.get().is_empty() {
                            "Press Enter to apply. Example: control+alt+Space".to_owned()
                        } else {
                            hotkey_status.get()
                        }
                    })
                    .text_size(px(12.0))
                    .text_color(rgb(steward_ui_components::palette::MUTED_FOREGROUND)),
                ),
        ))
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let value = value.trim().trim_start_matches('#');
    (value.len() == 6)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

fn section(title: impl Into<String>, body: impl IntoElement) -> impl IntoElement {
    view()
        .gap_3()
        .p_4()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(steward_ui_components::palette::BORDER))
        .child(
            text(title.into())
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(steward_ui_components::palette::FOREGROUND)),
        )
        .child(body)
}
