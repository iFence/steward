//! GPUI Kit settings window.

use std::{rc::Rc, sync::Arc};

use gpui::{
    div, px, rgb, App, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    Keystroke, ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement,
    Styled, Window,
};
use gpui_kit::{
    assets::IconName,
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        text::TextView,
        v_flex, ActiveTheme, Disableable as _, Icon, Theme, ThemeMode,
    },
    prelude::FluentBuilder,
};

use crate::{
    developer_diagnostics::DeveloperDiagnosticsPort,
    i18n::Localization,
    plugin_management::{PluginManagementPort, PluginManagementSnapshot},
    services::{AppServices, WindowCommand},
    updates::UpdateState,
};
use steward_plugin_registry::PluginPreferenceKind;
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
    check_updates_on_startup: bool,
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
        let check_updates_on_startup = services.check_updates_on_startup();
        let hotkey_focus = cx.focus_handle();
        services.plugin_management().refresh();

        Self {
            services,
            i18n,
            language,
            accent,
            autostart,
            summon_hotkey: hotkey,
            check_updates_on_startup,
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
        let updates_value = view.clone();
        let updates_set = view.clone();
        let updates_services = services.clone();
        let accent_value = view.clone();
        let accent_set = view.clone();
        let hotkey_field = view.clone();
        let hotkey_recording_label =
            SharedString::from(self.i18n.translate("settings-hotkey-recording"));
        let plugin_port = services.plugin_management();
        let plugin_snapshot = plugin_port.snapshot();
        let developer_diagnostics = services.developer_diagnostics();
        let plugin_i18n = self.i18n.clone();
        let plugin_preference_groups =
            render_plugin_preferences(plugin_port.clone(), &plugin_snapshot);
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
                        ))
                        .item(SettingItem::new(
                            self.i18n.translate("settings-update-check-on-startup"),
                            SettingField::switch(
                                move |cx| updates_value.read(cx).check_updates_on_startup,
                                move |enabled, cx| {
                                    updates_services.send_window_command(
                                        WindowCommand::SetCheckUpdatesOnStartup(enabled),
                                    );
                                    updates_set.update(cx, |settings, cx| {
                                        settings.check_updates_on_startup = enabled;
                                        cx.notify();
                                    });
                                },
                            )
                            .default_value(true),
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
                            SharedString::from(hotkey_display(&settings.summon_hotkey))
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
            SettingPage::new(self.i18n.translate("settings-plugins"))
                .icon(Icon::new(IconName::Puzzle))
                .group(SettingGroup::new().item(SettingItem::render({
                    let port = plugin_port;
                    let snapshot = plugin_snapshot;
                    let developer_diagnostics = developer_diagnostics.clone();
                    let i18n = plugin_i18n;
                    move |_options, _window, cx| {
                        render_plugins(
                            port.clone(),
                            snapshot.clone(),
                            developer_diagnostics.clone(),
                            i18n.clone(),
                            cx,
                        )
                    }
                })))
                .groups(plugin_preference_groups),
            SettingPage::new(self.i18n.translate("settings-about"))
                .resettable(false)
                .icon(Icon::new(IconName::Info))
                .group(SettingGroup::new().item(SettingItem::render({
                    let services = services.clone();
                    let i18n = self.i18n.clone();
                    move |_options, _window, cx| {
                        render_about_updates(services.clone(), i18n.clone(), cx)
                    }
                }))),
        ])
    }
}

fn render_plugin_preferences(
    port: PluginManagementPort,
    snapshot: &PluginManagementSnapshot,
) -> Vec<SettingGroup> {
    snapshot
        .catalog
        .installed
        .iter()
        .filter(|plugin| !plugin.manifest.preferences.is_empty())
        .map(|plugin| {
            let plugin_id = plugin.manifest.id.clone();
            let items = plugin
                .manifest
                .preferences
                .fields
                .iter()
                .map(|field| {
                    let description = field
                        .description
                        .clone()
                        .unwrap_or_else(|| field.key.clone());
                    let field_item = match field.kind {
                        PluginPreferenceKind::Boolean => {
                            let read_port = port.clone();
                            let write_port = port.clone();
                            let read_plugin = plugin_id.clone();
                            let write_plugin = plugin_id.clone();
                            let read_key = field.key.clone();
                            let write_key = field.key.clone();
                            SettingItem::new(
                                field.title.clone(),
                                SettingField::switch(
                                    move |_| {
                                        read_port
                                            .snapshot()
                                            .preferences
                                            .get(&read_plugin)
                                            .and_then(|values| values.get(&read_key))
                                            .and_then(serde_json::Value::as_bool)
                                            .unwrap_or(false)
                                    },
                                    move |enabled, _| {
                                        write_port.set_preference(
                                            write_plugin.clone(),
                                            write_key.clone(),
                                            serde_json::json!(enabled),
                                        );
                                    },
                                )
                                .default_value(false),
                            )
                        }
                        PluginPreferenceKind::Select => {
                            let options = field
                                .options
                                .iter()
                                .map(|option| {
                                    (
                                        SharedString::from(option.value.clone()),
                                        SharedString::from(option.label.clone()),
                                    )
                                })
                                .collect::<Vec<_>>();
                            let read_port = port.clone();
                            let write_port = port.clone();
                            let read_plugin = plugin_id.clone();
                            let write_plugin = plugin_id.clone();
                            let read_key = field.key.clone();
                            let write_key = field.key.clone();
                            let default = field
                                .default
                                .as_ref()
                                .and_then(serde_json::Value::as_str)
                                .or_else(|| {
                                    field.options.first().map(|option| option.value.as_str())
                                })
                                .unwrap_or_default()
                                .to_owned();
                            SettingItem::new(
                                field.title.clone(),
                                SettingField::dropdown(
                                    options,
                                    move |_| {
                                        SharedString::from(
                                            read_port
                                                .snapshot()
                                                .preferences
                                                .get(&read_plugin)
                                                .and_then(|values| values.get(&read_key))
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or_default(),
                                        )
                                    },
                                    move |value, _| {
                                        write_port.set_preference(
                                            write_plugin.clone(),
                                            write_key.clone(),
                                            serde_json::json!(value.to_string()),
                                        );
                                    },
                                )
                                .default_value(SharedString::from(default)),
                            )
                        }
                        PluginPreferenceKind::Text
                        | PluginPreferenceKind::Folder
                        | PluginPreferenceKind::Application => {
                            let read_port = port.clone();
                            let write_port = port.clone();
                            let read_plugin = plugin_id.clone();
                            let write_plugin = plugin_id.clone();
                            let read_key = field.key.clone();
                            let write_key = field.key.clone();
                            let default = field
                                .default
                                .as_ref()
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            SettingItem::new(
                                field.title.clone(),
                                SettingField::input(
                                    move |_| {
                                        SharedString::from(
                                            read_port
                                                .snapshot()
                                                .preferences
                                                .get(&read_plugin)
                                                .and_then(|values| values.get(&read_key))
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or_default(),
                                        )
                                    },
                                    move |value, _| {
                                        write_port.set_preference(
                                            write_plugin.clone(),
                                            write_key.clone(),
                                            serde_json::json!(value.to_string()),
                                        );
                                    },
                                )
                                .default_value(SharedString::from(default)),
                            )
                        }
                    };
                    field_item.description(description)
                })
                .collect::<Vec<_>>();
            SettingGroup::new()
                .title(plugin.manifest.name.clone())
                .items(items)
        })
        .collect()
}

fn render_plugins(
    port: PluginManagementPort,
    snapshot: PluginManagementSnapshot,
    developer_diagnostics: DeveloperDiagnosticsPort,
    i18n: Rc<Localization>,
    cx: &mut App,
) -> impl IntoElement {
    let refresh_port = port.clone();
    let sync_port = port.clone();
    let local_zip_port = port.clone();
    let local_directory_port = port.clone();
    let developer_mode = developer_diagnostics.is_enabled();
    let busy = snapshot.busy;
    let mut content = v_flex()
        .gap_3()
        .w_full()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .child(
                    Button::new("plugins-install-local-zip")
                        .label(i18n.translate("settings-plugin-install-zip"))
                        .outline()
                        .compact()
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            prompt_local_plugin(local_zip_port.clone(), false, cx);
                        }),
                )
                .child(
                    Button::new("plugins-install-local-directory")
                        .label(i18n.translate("settings-plugin-install-directory"))
                        .outline()
                        .compact()
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            prompt_local_plugin(local_directory_port.clone(), true, cx);
                        }),
                )
                .child(
                    Button::new("plugins-refresh")
                        .label(i18n.translate("settings-plugin-refresh"))
                        .outline()
                        .compact()
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, _, _| refresh_port.refresh()),
                )
                .child(
                    Button::new("plugins-sync-marketplace")
                        .label(i18n.translate("settings-plugin-sync-marketplace"))
                        .outline()
                        .compact()
                        .disabled(busy)
                        .on_click(move |_, _, _| sync_port.sync_marketplace()),
                ),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(i18n.translate("settings-plugins-description")),
        );

    if let Some(message) = &snapshot.message {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().primary)
                .child(message.clone()),
        );
    }
    if let Some(error) = &snapshot.error {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().danger)
                .child(error.clone()),
        );
    }
    for error in &snapshot.catalog.marketplace_errors {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().danger)
                .child(error.clone()),
        );
    }
    if let Some(revocations) = &snapshot.catalog.revocations {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "{} {}",
                    i18n.translate("settings-plugin-revocations"),
                    revocations.sequence
                )),
        );
    }

    content = content.child(
        div()
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(i18n.translate("settings-plugin-installed")),
    );
    if snapshot.catalog.installed.is_empty() {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(i18n.translate("settings-plugin-no-plugins")),
        );
    }
    for plugin in &snapshot.catalog.installed {
        let plugin_id = plugin.manifest.id.clone();
        let capabilities = snapshot
            .capabilities
            .get(&plugin_id)
            .cloned()
            .unwrap_or_default();
        let (status, status_color) = match &plugin.inspection {
            Some(inspection) if inspection.trusted && inspection.policy.passed => (
                i18n.translate("settings-plugin-trusted"),
                cx.theme().primary,
            ),
            Some(inspection) if inspection.trusted => (
                i18n.translate("settings-plugin-policy-failed"),
                cx.theme().danger,
            ),
            Some(inspection) if inspection.signature_valid => (
                i18n.translate("settings-plugin-untrusted"),
                cx.theme().danger,
            ),
            _ => (
                i18n.translate("settings-plugin-invalid-signature"),
                cx.theme().danger,
            ),
        };
        let publisher = plugin
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.publisher_key_id.clone())
            .unwrap_or_else(|| i18n.translate("settings-plugin-unknown-publisher"));
        let package_sha = plugin
            .inspection
            .as_ref()
            .map(|inspection| {
                inspection
                    .package_sha256
                    .chars()
                    .take(16)
                    .collect::<String>()
            })
            .unwrap_or_default();
        let update = snapshot
            .catalog
            .marketplace
            .entries
            .iter()
            .filter(|entry| entry.id == plugin.manifest.id)
            .max_by(|left, right| {
                compare_versions(&left.version, &right.version)
                    .then_with(|| left.build.cmp(&right.build))
            })
            .filter(|entry| compare_versions(&entry.version, &plugin.manifest.version).is_gt())
            .cloned();
        let update_port = port.clone();
        let remove_port = port.clone();
        let rollback_port = port.clone();
        let reload_port = port.clone();
        let update_id = plugin_id.clone();
        let rollback_id = plugin_id.clone();
        let remove_id = plugin_id.clone();
        let reload_id = plugin_id.clone();
        let rollback_version = plugin
            .history
            .first()
            .map(|history| history.version.as_str())
            .unwrap_or_default();
        let mut actions = h_flex().gap_2();
        if update.is_some() {
            actions = actions.child(
                Button::new(format!("plugin-update-{plugin_id}"))
                    .label(i18n.translate("settings-plugin-update"))
                    .primary()
                    .compact()
                    .disabled(busy)
                    .on_click(move |_, _, _| update_port.update(update_id.clone())),
            );
        }
        if !rollback_version.is_empty() {
            let label = format!(
                "{} {rollback_version}",
                i18n.translate("settings-plugin-rollback")
            );
            actions = actions.child(
                Button::new(format!("plugin-rollback-{plugin_id}"))
                    .label(label)
                    .outline()
                    .compact()
                    .disabled(busy)
                    .on_click(move |_, _, _| rollback_port.rollback(rollback_id.clone())),
            );
        }
        actions = actions.child(
            Button::new(format!("plugin-remove-{plugin_id}"))
                .label(i18n.translate("settings-plugin-remove"))
                .outline()
                .compact()
                .disabled(busy)
                .on_click(move |_, _, _| remove_port.remove(remove_id.clone())),
        );
        if developer_mode {
            actions = actions.child(
                Button::new(format!("plugin-reload-{plugin_id}"))
                    .label(i18n.translate("settings-plugin-reload"))
                    .outline()
                    .compact()
                    .disabled(busy)
                    .on_click(move |_, _, _| reload_port.reload_developer(&reload_id)),
            );
        }
        content = content.child(
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted)
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(format!(
                                            "{} {}",
                                            plugin.manifest.name, plugin.manifest.version
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{}: {publisher}",
                                            i18n.translate("settings-plugin-publisher")
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(status_color)
                                .child(status),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{}: {}",
                            i18n.translate("settings-plugin-package-hash"),
                            if package_sha.is_empty() {
                                "-"
                            } else {
                                package_sha.as_str()
                            }
                        )),
                )
                .when_some(plugin.error.clone(), |this, error| {
                    this.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .when(
                    plugin
                        .inspection
                        .as_ref()
                        .is_some_and(|inspection| !inspection.policy.warnings.is_empty()),
                    |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} {}",
                                    i18n.translate("settings-plugin-policy-review"),
                                    plugin
                                        .inspection
                                        .as_ref()
                                        .map(|inspection| inspection.policy.warnings.len())
                                        .unwrap_or(0)
                                )),
                        )
                    },
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(i18n.translate("settings-plugin-capabilities")),
                )
                .when(capabilities.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(cx.theme().muted_foreground)
                            .child(i18n.translate("settings-plugin-no-capabilities")),
                    )
                })
                .children(capabilities.into_iter().map(|capability| {
                    let allowed = capability.allowed();
                    let grant_ids = capability.grant_ids;
                    let automatic = capability.automatic;
                    let legacy_compatible = capability.legacy_compatible;
                    let release_allowed = capability.release_allowed;
                    let descriptor = capability.descriptor;
                    let (capability_status, capability_color) = if !release_allowed {
                        (
                            i18n.translate("settings-plugin-capability-blocked"),
                            cx.theme().danger,
                        )
                    } else if automatic {
                        (
                            i18n.translate("settings-plugin-capability-automatic"),
                            cx.theme().muted_foreground,
                        )
                    } else if legacy_compatible {
                        (
                            i18n.translate("settings-plugin-capability-legacy"),
                            cx.theme().primary,
                        )
                    } else if allowed {
                        (
                            i18n.translate("settings-plugin-capability-granted"),
                            cx.theme().primary,
                        )
                    } else {
                        (
                            i18n.translate("settings-plugin-capability-denied"),
                            cx.theme().danger,
                        )
                    };
                    let action = if descriptor.sensitive
                        && release_allowed
                        && !legacy_compatible
                        && !automatic
                    {
                        if grant_ids.is_empty() {
                            let grant_port = port.clone();
                            let plugin_id = plugin_id.clone();
                            let permission = descriptor.permission.clone();
                            let scope = descriptor.scope.clone();
                            let button_id = format!(
                                "plugin-grant-{plugin_id}-{}-{}",
                                descriptor.permission, descriptor.scope
                            );
                            Some(
                                Button::new(button_id)
                                    .label(i18n.translate("settings-plugin-capability-allow"))
                                    .primary()
                                    .compact()
                                    .disabled(busy)
                                    .on_click(move |_, _, _| {
                                        grant_port.grant_capability(
                                            plugin_id.clone(),
                                            permission.clone(),
                                            scope.clone(),
                                        )
                                    }),
                            )
                        } else {
                            let revoke_port = port.clone();
                            let plugin_id = plugin_id.clone();
                            let permission = descriptor.permission.clone();
                            let scope = descriptor.scope.clone();
                            let button_id = format!(
                                "plugin-revoke-{plugin_id}-{}-{}",
                                descriptor.permission, descriptor.scope
                            );
                            Some(
                                Button::new(button_id)
                                    .label(i18n.translate("settings-plugin-capability-revoke"))
                                    .outline()
                                    .compact()
                                    .disabled(busy)
                                    .on_click(move |_, _, _| {
                                        revoke_port.revoke_capability(
                                            plugin_id.clone(),
                                            permission.clone(),
                                            scope.clone(),
                                        )
                                    }),
                            )
                        }
                    } else {
                        None
                    };
                    v_flex()
                        .w_full()
                        .gap_1()
                        .p_2()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .justify_between()
                                .gap_2()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(descriptor.permission.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(descriptor.scope.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(capability_color)
                                        .child(capability_status),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(descriptor.description),
                        )
                        .when_some(action, |this, action| {
                            this.child(h_flex().justify_end().child(action))
                        })
                }))
                .child(actions),
        );
    }

    content = content.child(
        div()
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(i18n.translate("settings-plugin-marketplace")),
    );
    let installed_ids = snapshot
        .catalog
        .installed
        .iter()
        .map(|plugin| plugin.manifest.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut available = snapshot
        .catalog
        .marketplace
        .entries
        .iter()
        .filter(|entry| !installed_ids.contains(entry.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    available.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| compare_versions(&right.version, &left.version))
    });
    if available.is_empty() {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(i18n.translate("settings-plugin-marketplace-empty")),
        );
    }
    for entry in available {
        let install_port = port.clone();
        let plugin_id = entry.id.clone();
        content = content.child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .p_3()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{} {}", entry.name, entry.version)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} {}",
                                    i18n.translate("settings-plugin-publisher"),
                                    entry.publisher_key_id
                                )),
                        ),
                )
                .child(
                    Button::new(format!("plugin-install-{plugin_id}"))
                        .label(i18n.translate("settings-plugin-install"))
                        .primary()
                        .compact()
                        .disabled(busy)
                        .on_click(move |_, _, _| install_port.install(plugin_id.clone())),
                ),
        );
    }

    if developer_mode {
        content = content.child(render_developer_diagnostics(
            port,
            snapshot,
            developer_diagnostics,
            i18n,
            cx,
        ));
    }

    content
}

fn prompt_local_plugin(port: PluginManagementPort, directory: bool, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: !directory,
        directories: directory,
        multiple: false,
        prompt: Some(if directory {
            "Select plugin package folder".into()
        } else {
            "Select plugin package ZIP".into()
        }),
    });
    cx.spawn(async move |_| match paths.await {
        Ok(Ok(Some(paths))) => {
            if let Some(path) = paths.into_iter().next() {
                port.install_path(path);
            }
        }
        Ok(Ok(None)) => {}
        Ok(Err(error)) => port.show_error(format!("Open the plugin picker: {error}")),
        Err(_) => port.show_error("The plugin picker closed unexpectedly.".into()),
    })
    .detach();
}

fn render_developer_diagnostics(
    port: PluginManagementPort,
    snapshot: PluginManagementSnapshot,
    developer_diagnostics: DeveloperDiagnosticsPort,
    i18n: Rc<Localization>,
    cx: &mut App,
) -> impl IntoElement {
    let diagnostics = developer_diagnostics.snapshot();
    let mut content = v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(i18n.translate("settings-plugin-developer-diagnostics")),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "{} {}",
                    i18n.translate("settings-plugin-developer-data-dir"),
                    diagnostics.data_dir.display()
                )),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(cx.theme().muted_foreground)
                .child(i18n.translate("settings-plugin-developer-payloads-omitted")),
        );

    for plugin in &snapshot.catalog.installed {
        let runtime = diagnostics
            .plugins
            .iter()
            .find(|diagnostic| diagnostic.plugin_id == plugin.manifest.id);
        let audit = snapshot
            .capability_audit
            .get(&plugin.manifest.id)
            .cloned()
            .unwrap_or_default();
        let host_failures = audit
            .iter()
            .filter(|entry| {
                matches!(
                    entry.outcome.as_str(),
                    "denied" | "failed" | "permission_denied" | "host_failure"
                )
            })
            .count();
        let denied = audit
            .iter()
            .filter(|entry| matches!(entry.outcome.as_str(), "denied" | "permission_denied"))
            .count();
        let first_frame = runtime
            .and_then(|runtime| runtime.first_frame_ms)
            .map(|milliseconds| format!("{milliseconds} ms"))
            .unwrap_or_else(|| "-".into());
        let exits = runtime.map_or(0, |runtime| runtime.process_exits);
        let crashes = runtime.map_or(0, |runtime| runtime.crashes);
        let reload_port = port.clone();
        let plugin_id = plugin.manifest.id.clone();
        let mut plugin_content = v_flex()
            .w_full()
            .gap_2()
            .p_2()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!(
                                        "{} {}",
                                        plugin.manifest.name, plugin.manifest.version
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(plugin.manifest.id.clone()),
                            ),
                    )
                    .child(
                        Button::new(format!("developer-reload-{plugin_id}"))
                            .label(i18n.translate("settings-plugin-reload"))
                            .outline()
                            .compact()
                            .on_click(move |_, _, _| reload_port.reload_developer(&plugin_id)),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .text_size(px(10.0))
                    .child(format!(
                        "{}: {first_frame}",
                        i18n.translate("settings-plugin-developer-first-frame")
                    ))
                    .child(format!(
                        "{}: {host_failures}",
                        i18n.translate("settings-plugin-developer-host-failures")
                    ))
                    .child(format!(
                        "{}: {denied}",
                        i18n.translate("settings-plugin-developer-denied")
                    ))
                    .child(format!(
                        "{}: {exits}",
                        i18n.translate("settings-plugin-developer-exits")
                    ))
                    .child(format!(
                        "{}: {crashes}",
                        i18n.translate("settings-plugin-developer-crashes")
                    )),
            );
        if !audit.is_empty() {
            plugin_content = plugin_content.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(i18n.translate("settings-plugin-developer-traces")),
                    )
                    .children(audit.iter().rev().take(8).map(|entry| {
                        div()
                            .text_size(px(10.0))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{}  {}", entry.capability, entry.outcome))
                    })),
            );
        }
        if let Some(runtime) = runtime {
            if let Some(error) = &runtime.last_error {
                plugin_content = plugin_content.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                );
            }
            if !runtime.recent_events.is_empty() {
                plugin_content = plugin_content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(i18n.translate("settings-plugin-developer-traces")),
                        )
                        .children(runtime.recent_events.iter().rev().take(8).map(|event| {
                            div()
                                .text_size(px(10.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(event.clone())
                        })),
                );
            }
            if !runtime.log_excerpt.is_empty() {
                plugin_content = plugin_content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(i18n.translate("settings-plugin-developer-logs")),
                        )
                        .children(runtime.log_excerpt.iter().rev().take(8).map(|line| {
                            div()
                                .text_size(px(10.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(line.clone())
                        })),
                );
            }
        }
        content = content.child(plugin_content);
    }
    content
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(left), semver::Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn render_about_updates(
    services: Arc<AppServices>,
    i18n: Rc<Localization>,
    cx: &mut App,
) -> impl IntoElement {
    let state = services.update_port().state();
    let busy = state.is_busy();
    let has_update = state.has_update();
    let version = format!(
        "{} {}",
        i18n.translate("settings-version"),
        env!("CARGO_PKG_VERSION")
    );

    let status = match &state {
        UpdateState::Idle => (
            i18n.translate("settings-update-idle"),
            cx.theme().muted_foreground,
        ),
        UpdateState::Checking => (
            i18n.translate("settings-checking-for-updates"),
            cx.theme().muted_foreground,
        ),
        UpdateState::Available { latest_version, .. } => (
            format!(
                "{} v{latest_version}",
                i18n.translate("settings-update-available")
            ),
            cx.theme().primary,
        ),
        UpdateState::Downloading {
            downloaded_bytes,
            total_bytes,
            ..
        } => {
            let percent = total_bytes
                .filter(|total| *total > 0)
                .map(|total| {
                    ((*downloaded_bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u32
                })
                .unwrap_or(0);
            (
                format!(
                    "{} {percent}%",
                    i18n.translate("settings-downloading-update")
                ),
                cx.theme().muted_foreground,
            )
        }
        UpdateState::Installing => (
            i18n.translate("settings-installing-update"),
            cx.theme().muted_foreground,
        ),
        UpdateState::UpToDate => (
            i18n.translate("settings-up-to-date"),
            cx.theme().muted_foreground,
        ),
        UpdateState::Error(message) => (
            format!(
                "{}: {message}",
                i18n.translate("settings-update-check-failed")
            ),
            cx.theme().danger,
        ),
    };

    let progress = if let UpdateState::Downloading {
        downloaded_bytes,
        total_bytes,
        ..
    } = &state
    {
        total_bytes
            .filter(|total| *total > 0)
            .map(|total| (*downloaded_bytes as f32 / total as f32).clamp(0.0, 1.0))
    } else {
        None
    };

    let release_notes = if let UpdateState::Available { release_notes, .. } = &state {
        (!release_notes.trim().is_empty()).then(|| release_notes.clone())
    } else {
        None
    };

    let check_services = services.clone();
    let install_services = services;
    let mut buttons = h_flex().gap_2();
    buttons = buttons.child(
        Button::new("settings-check-updates")
            .label(i18n.translate("settings-check-for-updates"))
            .outline()
            .compact()
            .loading(matches!(state, UpdateState::Checking))
            .disabled(busy)
            .on_click(move |_, _, _| {
                check_services.update_port().check_for_updates(true);
            }),
    );
    if has_update {
        buttons = buttons.child(
            Button::new("settings-download-update")
                .label(i18n.translate("settings-download-and-install"))
                .primary()
                .compact()
                .on_click(move |_, _, _| {
                    install_services
                        .update_port()
                        .download_and_install(install_services.clone());
                }),
        );
    }

    v_flex()
        .gap_3()
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
                .child(version),
        )
        .child(buttons)
        .child(
            div()
                .text_size(px(13.0))
                .text_color(status.1)
                .child(status.0),
        )
        .when_some(progress, |this, progress| {
            this.child(
                div()
                    .w(px(280.0))
                    .h(px(6.0))
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().border)
                    .child(
                        div()
                            .h_full()
                            .w(px((progress * 280.0).max(2.0)))
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().primary),
                    ),
            )
        })
        .when_some(release_notes, |this, notes| {
            this.child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(i18n.translate("settings-release-notes")),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_h(px(190.0))
                            .overflow_y_scrollbar()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .p_3()
                            .child(
                                TextView::markdown("settings-update-release-notes", notes)
                                    .w_full()
                                    .selectable(true),
                            ),
                    ),
            )
        })
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

fn hotkey_display(value: &str) -> String {
    value.to_ascii_uppercase()
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
    use gpui::{AssetSource as _, Modifiers};

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

    #[test]
    fn plugin_page_lucide_icon_is_bundled() {
        assert_eq!(IconName::Puzzle.path(), "icons/puzzle.svg");
        assert!(gpui_kit::assets::AllAssets
            .load("icons/puzzle.svg")
            .unwrap()
            .is_some());
    }

    #[test]
    fn hotkey_display_uses_uppercase_key_names() {
        assert_eq!(hotkey_display("alt+Space"), "ALT+SPACE");
        assert_eq!(hotkey_display("control+alt+Space"), "CONTROL+ALT+SPACE");
    }
}
