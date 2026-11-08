//! GPUI Kit settings window.

use std::{rc::Rc, sync::Arc};

use gpui::{
    div, px, rgb, App, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    Keystroke, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_kit::{
    assets::IconName,
    component::{
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        v_flex, ActiveTheme, Icon, Theme, ThemeMode,
    },
    prelude::FluentBuilder,
};

use crate::{
    i18n::Localization,
    services::{AppServices, WindowCommand},
};
use steward_ui_components::palette;

const LANGUAGES: [(&str, &str); 7] = [
    ("en", "English"),
    ("zh", "中文"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ru", "Русский"),
];

const ACCENTS: [(&str, &str, u32); 5] = [
    ("#863bff", "settings-theme-violet", 0x863bff),
    ("#4f8cff", "settings-theme-blue", 0x4f8cff),
    ("#2fbf71", "settings-theme-jade", 0x2fbf71),
    ("#ff8a3d", "settings-theme-amber", 0xff8a3d),
    ("#e85d75", "settings-theme-rose", 0xe85d75),
];

pub(crate) struct StewardSettings {
    services: Arc<AppServices>,
    i18n: Rc<Localization>,
    language: String,
    accent: String,
    autostart: bool,
    summon_hotkey: String,
    recording_hotkey: bool,
    hotkey_focus: FocusHandle,
    hotkey_status: SharedString,
}

impl StewardSettings {
    pub(crate) fn new(
        _window: &mut Window,
        cx: &mut Context<Self>,
        services: Arc<AppServices>,
        i18n: Rc<Localization>,
    ) -> Self {
        let language = services.locale();
        gpui_kit::component::set_locale(component_locale(&language));
        let accent = services.accent();
        let autostart = services.autostart();
        let hotkey = services.summon_hotkey();
        let hotkey_focus = cx.focus_handle();

        Self {
            services,
            i18n,
            language,
            accent,
            autostart,
            summon_hotkey: hotkey,
            recording_hotkey: false,
            hotkey_focus,
            hotkey_status: SharedString::default(),
        }
    }

    fn select_language(&mut self, language: String, cx: &mut Context<Self>) {
        gpui_kit::component::set_locale(component_locale(&language));
        self.language = language.clone();
        self.services
            .send_window_command(WindowCommand::SetLanguage(language));
        cx.notify();
    }

    fn select_accent(&mut self, accent: String, cx: &mut Context<Self>) {
        let Some(accent) = normalize_accent(&accent) else {
            return;
        };
        self.accent = accent.clone();
        self.services
            .send_window_command(WindowCommand::SetAccent(accent.clone()));
        apply_theme(cx, &accent);
        cx.notify();
    }

    fn record_hotkey(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        if matches!(keystroke.key.as_str(), "escape" | "tab") {
            self.recording_hotkey = false;
            self.hotkey_status = SharedString::default();
            cx.notify();
            return;
        }
        if is_modifier_key(&keystroke.key) {
            cx.notify();
            return;
        }
        let Some(value) = hotkey_from_keystroke(keystroke) else {
            self.hotkey_status = self.i18n.translate("settings-hotkey-invalid").into();
            self.recording_hotkey = false;
            cx.notify();
            return;
        };
        self.recording_hotkey = false;
        self.summon_hotkey = value.clone();
        self.services
            .send_window_command(WindowCommand::SetSummonHotkey(value));
        self.hotkey_status = SharedString::default();
        cx.notify();
    }
}

impl Render for StewardSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&format!("Steward {}", self.i18n.translate("app-settings")));
        let view = cx.entity();
        let services = self.services.clone();
        let language_options = LANGUAGES
            .iter()
            .map(|(code, name)| (SharedString::from(*code), SharedString::from(*name)))
            .collect::<Vec<_>>();
        let accent_options = ACCENTS
            .iter()
            .map(|(value, name, _)| {
                (
                    SharedString::from(*value),
                    SharedString::from(self.i18n.translate(name)),
                )
            })
            .collect::<Vec<_>>();

        let language_value = view.clone();
        let language_set = view.clone();
        let autostart_value = view.clone();
        let autostart_set = view.clone();
        let autostart_services = services.clone();
        let accent_value = view.clone();
        let accent_set = view.clone();
        let hotkey_field = view.clone();
        let hotkey_recording_label =
            SharedString::from(self.i18n.translate("settings-hotkey-recording"));
        let version = format!(
            "{} {}",
            self.i18n.translate("settings-version"),
            env!("CARGO_PKG_VERSION")
        );

        Settings::new(SharedString::from(format!(
            "steward-settings-{}",
            self.language
        )))
        .sidebar_width(px(190.0))
        .sidebar_size_range(px(160.0)..px(260.0))
        .pages(vec![
            SettingPage::new(self.i18n.translate("settings-general"))
                .default_open(true)
                .icon(Icon::new(IconName::Settings2))
                .group(
                    SettingGroup::new()
                        .item(SettingItem::new(
                            self.i18n.translate("settings-language"),
                            SettingField::dropdown(
                                language_options,
                                move |cx| {
                                    SharedString::from(language_value.read(cx).language.clone())
                                },
                                move |value, cx| {
                                    let language = value.to_string();
                                    language_set.update(cx, |settings, cx| {
                                        settings.select_language(language, cx);
                                    });
                                },
                            )
                            .default_value(SharedString::from("en")),
                        ))
                        .item(SettingItem::new(
                            self.i18n.translate("settings-launch-at-startup"),
                            SettingField::switch(
                                move |cx| autostart_value.read(cx).autostart,
                                move |enabled, cx| {
                                    autostart_services
                                        .send_window_command(WindowCommand::SetAutostart(enabled));
                                    autostart_set.update(cx, |settings, cx| {
                                        settings.autostart = enabled;
                                        cx.notify();
                                    });
                                },
                            )
                            .default_value(false),
                        )),
                ),
            SettingPage::new(self.i18n.translate("settings-appearance"))
                .icon(Icon::new(IconName::Palette))
                .group(
                    SettingGroup::new().item(SettingItem::new(
                        self.i18n.translate("settings-accent-color"),
                        SettingField::dropdown(
                            accent_options,
                            move |cx| SharedString::from(accent_value.read(cx).accent.clone()),
                            move |value, cx| {
                                let accent = value.to_string();
                                accent_set.update(cx, |settings, cx| {
                                    settings.select_accent(accent, cx);
                                });
                            },
                        )
                        .default_value(SharedString::from("#863bff")),
                    )),
                ),
            SettingPage::new(self.i18n.translate("settings-hotkeys"))
                .icon(Icon::default().data(include_bytes!("../../../assets/icons/keyboard.svg")))
                .group(SettingGroup::new().item(SettingItem::new(
                    self.i18n.translate("settings-summon-steward"),
                    SettingField::render(move |_options, window, cx| {
                        let settings = hotkey_field.read(cx);
                        let focus = settings.hotkey_focus.clone();
                        let recording = settings.recording_hotkey && focus.is_focused(window);
                        let value = if recording {
                            hotkey_recording_label.clone()
                        } else {
                            SharedString::from(settings.summon_hotkey.clone())
                        };
                        let status = settings.hotkey_status.clone();
                        let view = hotkey_field.clone();
                        let click_focus = focus.clone();
                        let click_view = view.clone();
                        let key_view = view.clone();
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .id("summon-hotkey-recorder")
                                    .h(px(34.0))
                                    .w(px(220.0))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(if recording {
                                        cx.theme().ring
                                    } else {
                                        cx.theme().border
                                    })
                                    .bg(cx.theme().muted)
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .track_focus(&focus)
                                    .tab_index(0)
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        click_focus.focus(window, cx);
                                        click_view.update(cx, |settings, cx| {
                                            settings.recording_hotkey = true;
                                            settings.hotkey_status = SharedString::default();
                                            cx.notify();
                                        });
                                    })
                                    .on_key_down(move |event, _, cx| {
                                        if event.is_held {
                                            return;
                                        }
                                        key_view.update(cx, |settings, cx| {
                                            settings.record_hotkey(&event.keystroke, cx);
                                        });
                                    })
                                    .child(value),
                            )
                            .when(!status.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(status),
                                )
                            })
                    }),
                ))),
            SettingPage::new(self.i18n.translate("settings-about"))
                .resettable(false)
                .icon(Icon::new(IconName::Info))
                .group(SettingGroup::new().item(SettingItem::render(
                    move |_options, _window, cx| {
                        v_flex()
                            .gap_2()
                            .w_full()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Steward"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(version.clone()),
                            )
                    },
                ))),
        ])
    }
}

pub(crate) fn init_ui(cx: &mut App, accent: &str) {
    gpui_kit::init(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    apply_theme(cx, accent);
}

pub(crate) fn apply_theme(cx: &mut App, accent: &str) {
    let accent = parse_hex_color(accent).unwrap_or(palette::ACCENT);
    Theme::update(cx, |theme| {
        theme.colors.background = rgb(palette::BACKGROUND).into();
        theme.colors.foreground = rgb(palette::FOREGROUND).into();
        theme.colors.border = rgb(palette::BORDER).into();
        theme.colors.input = rgb(palette::BORDER).into();
        theme.colors.group_box = rgb(palette::BACKGROUND_ALT).into();
        theme.colors.group_box_foreground = rgb(palette::FOREGROUND).into();
        theme.colors.muted = rgb(palette::BACKGROUND_ALT).into();
        theme.colors.muted_foreground = rgb(palette::MUTED_FOREGROUND).into();
        theme.colors.primary = rgb(accent).into();
        theme.colors.primary_active = rgb(accent).into();
        theme.colors.primary_hover = rgb(accent).into();
        theme.colors.ring = rgb(accent).into();
        theme.colors.caret = rgb(accent).into();
        theme.colors.switch = rgb(accent).into();
        theme.colors.sidebar = rgb(palette::BACKGROUND).into();
        theme.colors.sidebar_accent = rgb(palette::BACKGROUND_ALT).into();
        theme.colors.sidebar_accent_foreground = rgb(palette::FOREGROUND).into();
        theme.colors.sidebar_border = rgb(palette::BORDER).into();
        theme.colors.sidebar_foreground = rgb(palette::FOREGROUND).into();
        theme.colors.sidebar_primary = rgb(accent).into();
        theme.colors.sidebar_primary_foreground = rgb(palette::FOREGROUND).into();
        theme.colors.title_bar = rgb(palette::BACKGROUND).into();
        theme.colors.title_bar_border = rgb(palette::BORDER).into();
        theme.radius = px(8.0);
        theme.radius_lg = px(10.0);
    });
}

fn normalize_accent(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "control" | "ctrl" | "alt" | "shift" | "platform" | "super" | "win" | "function" | "fn"
    )
}

fn hotkey_from_keystroke(keystroke: &Keystroke) -> Option<String> {
    if is_modifier_key(&keystroke.key) {
        return None;
    }

    let has_modifier = keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.shift
        || keystroke.modifiers.platform;
    if !has_modifier && !is_function_key(&keystroke.key) {
        return None;
    }

    let mut parts = Vec::with_capacity(5);
    if keystroke.modifiers.control {
        parts.push("control");
    }
    if keystroke.modifiers.alt {
        parts.push("alt");
    }
    if keystroke.modifiers.shift {
        parts.push("shift");
    }
    if keystroke.modifiers.platform {
        parts.push("super");
    }
    parts.push(&keystroke.key);

    parts
        .join("+")
        .parse::<global_hotkey::hotkey::HotKey>()
        .ok()
        .map(|hotkey| hotkey.into_string())
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f').is_some_and(|number| {
        number
            .parse::<u8>()
            .is_ok_and(|number| (1..=24).contains(&number))
    })
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let value = value.trim().trim_start_matches('#');
    (value.len() == 6)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

fn component_locale(language: &str) -> &'static str {
    match language {
        "zh" => "zh-CN",
        _ => "en",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    #[test]
    fn recorded_keystroke_becomes_a_persistable_hotkey() {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                control: true,
                alt: true,
                ..Default::default()
            },
            key: "space".into(),
            key_char: None,
        };

        assert_eq!(
            hotkey_from_keystroke(&keystroke).as_deref(),
            Some("control+alt+Space")
        );
    }

    #[test]
    fn modifier_keys_do_not_replace_the_hotkey() {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: "control".into(),
            key_char: None,
        };

        assert_eq!(hotkey_from_keystroke(&keystroke), None);
    }

    #[test]
    fn plain_letters_are_not_global_hotkeys() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "a".into(),
            key_char: Some("a".into()),
        };

        assert_eq!(hotkey_from_keystroke(&keystroke), None);
    }

    #[test]
    fn function_keys_can_be_used_without_modifiers() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "f8".into(),
            key_char: None,
        };

        assert_eq!(hotkey_from_keystroke(&keystroke).as_deref(), Some("F8"));
    }
}
