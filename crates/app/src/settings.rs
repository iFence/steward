//! The settings window, built on gpui-component's `Settings` widget.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    div, prelude::*, px, size, Anchor, AnyWindowHandle, App, AsyncApp, Bounds, Entity,
    SharedString, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions,
};
use gpui_component::{
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex, ActiveTheme, Icon, IconName, Root, StyledExt,
};

use crate::autostart::{autostart_enabled, set_autostart};
use crate::config::{DEFAULT_ACCENT, LANGUAGE_SETTING, THEME_COLOR_SETTING};
use crate::hotkeys::{apply_hotkey, format_hotkey, keystroke_to_hotkey, HotkeyField};
use crate::i18n::Localization;
use crate::launcher::{LauncherState, StewardApp};
use crate::platform;
use crate::theme::{apply_steward_theme, parse_hex_color};

/// Settings window built on `gpui_component`'s `Settings` widget.
///
/// Pages:
/// - General: language, theme color and the launch-at-startup switch (the
///   single home for autostart; the tray menu no longer carries a duplicate
///   toggle).
/// - Hotkeys: global summon hotkey and the settings-window hotkey, each
///   recorded by pressing a combination inside the window.
/// - About: app name, version, and a short description.
struct SettingsApp {
    i18n: Rc<Localization>,
    storage: Rc<RefCell<steward_storage::Storage>>,
    state: Rc<RefCell<LauncherState>>,
    /// Current theme accent color as `0xRRGGBB`; drives the active swatch.
    accent: u32,
    /// Currently selected language code (e.g. `zh`), drives the dropdown.
    language: String,
    /// Which global-hotkey field is waiting for the next key combination.
    /// While set, the window's keystroke interceptor turns the next valid
    /// combination into the new hotkey for that field.
    recording: Option<HotkeyField>,
    /// Keeps the keystroke interceptor alive for the window's lifetime; the
    /// subscription is dropped (and the interceptor unregistered) when the
    /// settings window closes.
    _hotkey_subscription: Option<gpui::Subscription>,
}

/// Preset accent colors offered by the settings page. The default (violet) is
/// Tinycast's brand hue; the rest are pleasant neighbors on the surface.
const ACCENT_PRESETS: [u32; 5] = [
    0x863bff, // violet (default)
    0x89b4fa, // sea blue
    0x94e2d5, // jade
    0xf38ba8, // rose
    0xf9e2af, // amber
];
/// Languages offered in the settings dropdown: (Fluent resource code, native
/// display name). Native names are correct in every language, so they need no
/// translation.
const SUPPORTED_LANGUAGES: [(&str, &str); 7] = [
    ("zh", "中文"),
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

impl gpui::Render for SettingsApp {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let view_autostart = view.clone();
        let view_theme_value = view.clone();
        let view_theme_set = view.clone();
        let view_language_value = view.clone();
        let view_language_set = view.clone();
        let view_hotkey = view.clone();
        let view_hotkey_toggle = view.clone();
        let view_settings_hotkey = view.clone();
        let view_settings_toggle = view.clone();
        let i18n = self.i18n.clone();
        let i18n_hotkey = i18n.clone();
        let i18n_settings = i18n.clone();
        let state = self.state.clone();
        let state_hotkey = self.state.clone();
        let state_settings = self.state.clone();
        let storage = self.storage.clone();
        let storage_language = storage.clone();
        let general_title = self.i18n.translate("settings-general");
        let autostart_label = self.i18n.translate("app-autostart");
        let theme_title = self.i18n.translate("settings-theme");
        let language_title = self.i18n.translate("settings-language");
        let hotkeys_title = self.i18n.translate("settings-hotkeys");
        let global_hotkey_title = self.i18n.translate("settings-global-hotkey");
        let settings_hotkey_title = self.i18n.translate("settings-settings-hotkey");
        let about_title = self.i18n.translate("settings-about");
        let version_label = self.i18n.translate("settings-version");
        let language_options: Vec<(SharedString, SharedString)> = SUPPORTED_LANGUAGES
            .iter()
            .map(|(code, name)| (SharedString::from(*code), SharedString::from(*name)))
            .collect();
        let theme_options: Vec<(SharedString, SharedString)> = ACCENT_PRESETS
            .iter()
            .map(|&color| {
                (
                    SharedString::from(format!("#{color:06x}")),
                    SharedString::from(self.i18n.translate(accent_label_key(color))),
                )
            })
            .collect();

        // The keyed state id includes the language so switching locale
        // rebuilds the Settings widget's search input with a translated
        // placeholder.
        Settings::new(format!("steward-settings-{}", self.language))
            .sidebar_width(px(200.0))
            .sidebar_size_range(px(160.0)..px(280.0))
            .pages(vec![
                SettingPage::new(general_title)
                    .default_open(true)
                    .icon(Icon::new(IconName::Settings2))
                    .group(
                        SettingGroup::new()
                            .item(SettingItem::new(
                                language_title,
                                SettingField::render(move |_options, _window, cx| {
                                    let code = view_language_value.read(cx).language.clone();
                                    let current_label = SUPPORTED_LANGUAGES
                                        .iter()
                                        .find(|(c, _)| *c == code)
                                        .map(|(_, name)| SharedString::from(*name))
                                        .unwrap_or_else(|| SharedString::from(code.clone()));
                                    let on_select = {
                                        let storage_language = storage_language.clone();
                                        let i18n = i18n.clone();
                                        let state = state.clone();
                                        let view = view_language_set.clone();
                                        move |code: SharedString, cx: &mut App| {
                                            let _ = storage_language
                                                .borrow()
                                                .set_setting(LANGUAGE_SETTING, &code);
                                            i18n.select_language(&code);
                                            // Keep gpui-component's own widgets (the
                                            // settings search box) in sync, and
                                            // refresh the launcher's row type label.
                                            gpui_component::set_locale(gpui_component_locale(
                                                &code,
                                            ));
                                            update_launcher_label(&state, cx);
                                            view.update(cx, |app, cx| {
                                                app.language = code.to_string();
                                                cx.notify();
                                            });
                                        }
                                    };
                                    dropdown_field(
                                        "language-dropdown",
                                        SharedString::from(code),
                                        current_label,
                                        language_options.clone(),
                                        on_select,
                                    )
                                }),
                            ))
                            .item(SettingItem::new(
                                theme_title,
                                SettingField::render(move |_options, _window, cx| {
                                    let accent = view_theme_value.read(cx).accent;
                                    let value = SharedString::from(format!("#{accent:06x}"));
                                    let current_label = theme_options
                                        .iter()
                                        .find(|(option_value, _)| *option_value == value)
                                        .map(|(_, label)| label.clone())
                                        .unwrap_or_else(|| value.clone());
                                    let on_select = {
                                        let storage = storage.clone();
                                        let view = view_theme_set.clone();
                                        move |hex: SharedString, cx: &mut App| {
                                            if let Some(color) = parse_hex_color(&hex) {
                                                let _ = storage
                                                    .borrow()
                                                    .set_setting(THEME_COLOR_SETTING, &hex);
                                                apply_steward_theme(cx, color);
                                                view.update(cx, |app, cx| {
                                                    app.accent = color;
                                                    cx.notify();
                                                });
                                            }
                                        }
                                    };
                                    dropdown_field(
                                        "theme-dropdown",
                                        value,
                                        current_label,
                                        theme_options.clone(),
                                        on_select,
                                    )
                                }),
                            ))
                            .item(SettingItem::new(
                                autostart_label,
                                SettingField::switch(
                                    |_cx: &App| autostart_enabled(),
                                    move |enabled: bool, cx: &mut App| {
                                        // Write the registry, then re-render so the
                                        // switch reflects the state that actually
                                        // took effect.
                                        set_autostart(enabled);
                                        view_autostart.update(cx, |_, cx| cx.notify());
                                    },
                                )
                                .default_value(false),
                            )),
                    ),
                // Global shortcut keys live on their own page, not as a sub-group
                // of the General page: both hotkeys share one recording flow and
                // deserve a dedicated, self-contained settings surface.
                SettingPage::new(hotkeys_title)
                    .icon(Icon::new(IconName::Settings))
                    .group(
                        SettingGroup::new()
                            .item(SettingItem::new(
                                global_hotkey_title,
                                hotkey_setting_field(
                                    HotkeyField::Summon,
                                    view_hotkey,
                                    view_hotkey_toggle,
                                    state_hotkey,
                                    i18n_hotkey,
                                ),
                            ))
                            .item(SettingItem::new(
                                settings_hotkey_title,
                                hotkey_setting_field(
                                    HotkeyField::Settings,
                                    view_settings_hotkey,
                                    view_settings_toggle,
                                    state_settings,
                                    i18n_settings,
                                ),
                            )),
                    ),
                SettingPage::new(about_title)
                    .resettable(false)
                    .icon(Icon::new(IconName::Info))
                    .group(SettingGroup::new().item(SettingItem::render(
                        move |_options, _window, cx| {
                            v_flex()
                                .gap_3()
                                .w_full()
                                .items_center()
                                .child(
                                    Icon::new(IconName::GalleryVerticalEnd)
                                        .size_16()
                                        .text_color(cx.theme().primary),
                                )
                                .child(div().text_lg().font_semibold().child("Steward"))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{version_label} {}",
                                            env!("CARGO_PKG_VERSION")
                                        )),
                                )
                        },
                    ))),
            ])
    }
}

/// i18n key for an accent preset's localized name.
fn accent_label_key(color: u32) -> &'static str {
    match color {
        0x863bff => "settings-theme-violet",
        0x94e2d5 => "settings-theme-jade",
        0xf38ba8 => "settings-theme-rose",
        0xf9e2af => "settings-theme-amber",
        _ => "settings-theme-blue",
    }
}

/// Map a Steward language code to the gpui-component locale. The component
/// bundles only `en`/`zh-CN` (plus a few others); anything unsupported falls
/// back to English inside the component.
fn gpui_component_locale(code: &str) -> &'static str {
    match code {
        "zh" => "zh-CN",
        _ => "en",
    }
}

/// Refresh the launcher's localized row type label ("应用"/"Application")
/// after the UI language changes. The label is stored state in the results
/// list, so it needs an explicit update even though the shared i18n loader
/// already switched.
fn update_launcher_label(state: &Rc<RefCell<LauncherState>>, cx: &mut App) {
    let Some(window) = state.borrow().window else {
        return;
    };
    let Some(app) = window.downcast::<StewardApp>() else {
        return;
    };
    let _ = app.update(cx, |app, _window, cx| {
        let label = app.i18n.translate("application");
        app.results.set_type_label(label, cx);
        cx.notify();
    });
}

/// A settings dropdown control: a fixed-width outline button whose label is
/// centered (with a trailing caret), opening a popup menu of `options`.
/// Unlike gpui-component's built-in field dropdown, every instance has the
/// same width and the text is centered instead of left-aligned next to the
/// caret.
fn dropdown_field(
    id: &'static str,
    current_value: SharedString,
    current_label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    on_select: impl Fn(SharedString, &mut App) + 'static,
) -> impl IntoElement {
    let on_select = Rc::new(on_select);
    Button::new(id)
        .label(SharedString::from(format!("{current_label}  ▾")))
        .outline()
        .w(px(150.0))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let on_select = on_select.clone();
            options.iter().fold(menu, |menu, (value, label)| {
                let on_select = on_select.clone();
                let value = value.clone();
                menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(value == current_value)
                        .on_click(move |_, _, cx| on_select(value.clone(), cx)),
                )
            })
        })
        .into_any_element()
}

/// A settings field for a hotkey: a fixed-width outline button showing the
/// active binding, or the recording prompt while the window's keystroke
/// interceptor waits for the next combination. Clicking toggles recording for
/// `field` (the interceptor in `open_settings_window` applies the result).
/// The summon field edits a global hotkey; the settings field edits the
/// launcher-scoped settings hotkey.
fn hotkey_setting_field(
    field: HotkeyField,
    view: Entity<SettingsApp>,
    view_toggle: Entity<SettingsApp>,
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
) -> SettingField<SharedString> {
    SettingField::render(move |_options, _window, cx| {
        let recording = view.read(cx).recording == Some(field);
        let label = if recording {
            SharedString::from(i18n.translate("settings-hotkey-recording"))
        } else {
            let hotkey = field
                .active_hotkey(&state.borrow())
                .unwrap_or_else(|| field.default_hotkey());
            SharedString::from(format_hotkey(&hotkey))
        };
        let view_toggle = view_toggle.clone();
        Button::new(match field {
            HotkeyField::Summon => "summon-hotkey-button",
            HotkeyField::Settings => "settings-hotkey-button",
        })
        .label(label)
        .outline()
        .w(px(150.0))
        .on_click(move |_, _window, cx| {
            view_toggle.update(cx, |app, cx| {
                app.recording = if app.recording == Some(field) {
                    None
                } else {
                    Some(field)
                };
                cx.notify();
            });
        })
        .into_any_element()
    })
}

/// Open (or focus) the settings window. Keeps `LauncherState.settings_window`
/// in sync: the handle is cleared when the window is closed so a later menu
/// click reopens it instead of touching a stale handle.
fn open_settings_window(
    cx: &mut App,
    i18n: Rc<Localization>,
    state: &Rc<RefCell<LauncherState>>,
) -> AnyWindowHandle {
    // Wide enough that the settings page content stays above gpui-component's
    // 480px stacked-layout threshold with the default sidebar, keeping every
    // setting item on a single row (title left, control right).
    let bounds = Bounds::centered(None, size(px(800.0), px(480.0)), cx);
    let handle: AnyWindowHandle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Keep the settings window from shrinking below its default
                // size; the page content must stay above gpui-component's
                // 480px stacked-layout threshold for single-row items.
                window_min_size: Some(size(px(800.0), px(480.0))),
                titlebar: Some(TitlebarOptions {
                    appears_transparent: false,
                    ..Default::default()
                }),
                show: true,
                focus: true,
                kind: WindowKind::Normal,
                is_resizable: true,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title(&i18n.translate("app-settings"));
                // The native titlebar follows the OS theme by default; pin it
                // to the app's dark look (Windows light mode would otherwise
                // render it white).
                platform::force_dark_titlebar(window);
                // GPUI re-derives the titlebar color from the OS theme when
                // the appearance changes; re-pin it dark so toggling Windows
                // dark/light while the window is open keeps it black.
                window
                    .observe_window_appearance(|window, _cx| {
                        platform::force_dark_titlebar(window);
                    })
                    .detach();
                // The settings panel uses gpui-component widgets (input,
                // tooltip, menu popovers), so the window's root must be a
                // `Root`: it owns the overlay layers those widgets render into.
                let storage = state.borrow().storage.clone();
                let accent = storage
                    .borrow()
                    .get_setting(THEME_COLOR_SETTING)
                    .and_then(|value| parse_hex_color(&value))
                    .unwrap_or(DEFAULT_ACCENT);
                let language = storage
                    .borrow()
                    .get_setting(LANGUAGE_SETTING)
                    .unwrap_or_else(|| i18n.language());
                let view = cx.new(move |_| SettingsApp {
                    i18n,
                    storage,
                    state: state.clone(),
                    accent,
                    language,
                    recording: None,
                    _hotkey_subscription: None,
                });

                // While a hotkey field is recording, capture the next key
                // combination pressed in *this* window. The interceptor fires
                // before all other key handling, so the captured combo never
                // reaches the settings widget itself.
                let settings_window_handle = window.window_handle();
                let hotkey_view = view.clone();
                let hotkey_state = state.clone();
                let subscription = cx.intercept_keystrokes(move |event, window, cx| {
                    if window.window_handle() != settings_window_handle {
                        return;
                    }
                    let Some(field) = hotkey_view.read(cx).recording else {
                        return;
                    };
                    let mods = &event.keystroke.modifiers;
                    let has_modifier = mods.control || mods.alt || mods.shift || mods.platform;
                    // A bare Escape cancels recording; modifier-only presses
                    // are ignored by `keystroke_to_hotkey`.
                    if event.keystroke.key == "escape" && !has_modifier {
                        cx.stop_propagation();
                        hotkey_view.update(cx, |app, cx| {
                            app.recording = None;
                            cx.notify();
                        });
                        return;
                    }
                    let Some(hotkey) = keystroke_to_hotkey(&event.keystroke) else {
                        return;
                    };
                    cx.stop_propagation();
                    apply_hotkey(&hotkey_state, field, hotkey);
                    hotkey_view.update(cx, |app, cx| {
                        app.recording = None;
                        cx.notify();
                    });
                });
                view.update(cx, |app, _| app._hotkey_subscription = Some(subscription));

                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open the settings window")
        .into();

    let settings_id = handle.window_id();
    let closed_state = state.clone();
    cx.on_window_closed(move |_cx, window_id| {
        if window_id == settings_id {
            closed_state.borrow_mut().settings_window = None;
        }
    })
    .detach();

    handle
}

/// Show the settings window, focusing it if it is already open (reopening it
/// if it was closed). Shared by the tray menu and the settings global hotkey.
pub(crate) fn toggle_settings_window(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut AsyncApp,
) {
    cx.update(|cx| open_settings_window_from_launcher(state, i18n, cx));
}

/// Open (or focus) the settings window. Creates it on first use and keeps
/// `LauncherState.settings_window` in sync: the handle is cleared when the
/// window is closed, so a later call reopens it instead of touching a stale
/// handle. Used by the tray menu and the launcher-scoped settings hotkey.
pub(crate) fn open_settings_window_from_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut App,
) {
    let state_ref = state.borrow();
    if let Some(handle) = state_ref.settings_window.as_ref() {
        let _ = handle.update(cx, |_, window, cx| {
            cx.activate(true);
            window.refresh();
        });
    } else {
        drop(state_ref);
        let handle = open_settings_window(cx, i18n, state);
        state.borrow_mut().settings_window = Some(handle);
    }
}
