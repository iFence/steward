//! Global (and launcher-scoped) hotkey registration and conversion between
//! GPUI keystrokes and `global-hotkey` bindings.

use std::{cell::RefCell, rc::Rc};

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};
use gpui::Keystroke;

use crate::config::{SETTINGS_HOTKEY_SETTING, SUMMON_HOTKEY_SETTING};
use crate::launcher::LauncherState;

/// Which hotkey a settings field edits: `Summon` is registered globally and
/// works from anywhere; `Settings` is launcher-scoped (the launcher matches it
/// in its key handling while the bar is visible) and never registered globally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HotkeyField {
    /// Summons the launcher bar.
    Summon,
    /// Opens the settings window.
    Settings,
}

impl HotkeyField {
    /// Storage key persisting this hotkey as a `HotKey::into_string()` value.
    fn setting_key(self) -> &'static str {
        match self {
            HotkeyField::Summon => SUMMON_HOTKEY_SETTING,
            HotkeyField::Settings => SETTINGS_HOTKEY_SETTING,
        }
    }

    /// The currently registered hotkey for this field (`None` only when no
    /// binding could be registered at startup, e.g. it collided with another
    /// application).
    pub(crate) fn active_hotkey(self, state: &LauncherState) -> Option<HotKey> {
        match self {
            HotkeyField::Summon => state.summon_hotkey,
            HotkeyField::Settings => state.settings_hotkey,
        }
    }

    /// The built-in binding, used when nothing is persisted yet or the
    /// persisted value cannot be parsed.
    pub(crate) fn default_hotkey(self) -> HotKey {
        match self {
            HotkeyField::Summon => {
                HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
            }
            HotkeyField::Settings => HotKey::new(Some(Modifiers::CONTROL), Code::Comma),
        }
    }
}

/// Read the persisted summon hotkey, falling back to the default on a missing
/// or unparseable value.
fn read_summon_hotkey(state: &Rc<RefCell<LauncherState>>) -> HotKey {
    read_hotkey(state, HotkeyField::Summon)
}

/// Read the persisted settings-window hotkey, falling back to the default on a
/// missing or unparseable value.
fn read_settings_hotkey(state: &Rc<RefCell<LauncherState>>) -> HotKey {
    read_hotkey(state, HotkeyField::Settings)
}

fn read_hotkey(state: &Rc<RefCell<LauncherState>>, field: HotkeyField) -> HotKey {
    state
        .borrow()
        .storage
        .borrow()
        .get_setting(field.setting_key())
        .and_then(|value| value.parse::<HotKey>().ok())
        .unwrap_or_else(|| field.default_hotkey())
}

/// Create the global hotkey manager, register the persisted summon hotkey
/// (falling back to the default when registration fails), and store everything
/// in `state`. The manager is kept in `LauncherState` instead of leaked so the
/// settings window can re-register the summon hotkey later; its hidden window
/// delivers `WM_HOTKEY` to whichever message pump owns the event-loop thread.
/// The settings hotkey is launcher-scoped (never registered globally) and is
/// only stored so the launcher's key handling can match it.
pub(crate) fn setup_global_hotkey(state: &Rc<RefCell<LauncherState>>) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let mut summon = read_summon_hotkey(state);
    if let Err(error) = manager.register(summon) {
        eprintln!(
            "failed to register configured summon hotkey {summon}: {error:#}; \
             falling back to the default"
        );
        summon = HotkeyField::Summon.default_hotkey();
        manager
            .register(summon)
            .context("register default global hotkey")?;
    }
    // Read the persisted settings hotkey before the mutable borrow below, so
    // `read_hotkey` does not re-borrow the shared state.
    let settings = read_settings_hotkey(state);
    let mut state_ref = state.borrow_mut();
    state_ref.hotkey_manager = Some(manager);
    state_ref.summon_hotkey = Some(summon);
    state_ref.settings_hotkey = Some(settings);
    Ok(())
}

/// Replace the active hotkey for `field`. For the global summon hotkey:
/// unregister the old binding, register the new one, and persist it; when the
/// new binding cannot be registered (e.g. it is already taken by another
/// application) the old binding is restored and nothing is persisted. The
/// settings hotkey is launcher-scoped, so a change only persists the new
/// binding (the launcher matches it while the bar is visible). Returns whether
/// the change took effect.
pub(crate) fn apply_hotkey(
    state: &Rc<RefCell<LauncherState>>,
    field: HotkeyField,
    hotkey: HotKey,
) -> bool {
    let mut state_ref = state.borrow_mut();
    let previous = field.active_hotkey(&state_ref);
    if previous == Some(hotkey) {
        return true;
    }
    if matches!(field, HotkeyField::Settings) {
        let _ = state_ref
            .storage
            .borrow()
            .set_setting(field.setting_key(), &hotkey.to_string());
        state_ref.settings_hotkey = Some(hotkey);
        return true;
    }
    let Some(manager) = state_ref.hotkey_manager.as_ref() else {
        return false;
    };
    if let Some(previous) = previous {
        let _ = manager.unregister(previous);
    }
    match manager.register(hotkey) {
        Ok(()) => {
            let _ = state_ref
                .storage
                .borrow()
                .set_setting(field.setting_key(), &hotkey.to_string());
            state_ref.summon_hotkey = Some(hotkey);
            true
        }
        Err(error) => {
            eprintln!("failed to register {field:?} hotkey {hotkey}: {error:#}");
            if let Some(previous) = previous {
                let _ = manager.register(previous);
            }
            state_ref.summon_hotkey = previous;
            false
        }
    }
}

/// Convert a GPUI keystroke into a global `HotKey`. Requires at least one
/// modifier (control/alt/shift/super) so a global hotkey never hijacks a plain
/// key, and a main key that maps to a physical `Code` (modifier-only presses
/// and unmappable keys return `None`).
pub(crate) fn keystroke_to_hotkey(keystroke: &Keystroke) -> Option<HotKey> {
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    let mods = &keystroke.modifiers;
    if mods.control {
        parts.push("ctrl");
    }
    if mods.alt {
        parts.push("alt");
    }
    if mods.shift {
        parts.push("shift");
    }
    if mods.platform {
        parts.push("super");
    }
    if parts.is_empty() {
        return None;
    }
    let key = gpui_key_to_hotkey_token(&keystroke.key)?;
    format!("{}+{}", parts.join("+"), key).parse().ok()
}

/// Map a GPUI keystroke key string to the token `HotKey`'s parser accepts
/// (e.g. `"space"` -> `"Space"`, `"a"` -> `"A"`, `"f9"` -> `"F9"`). Returns
/// `None` for modifier-only and otherwise unmappable keys.
fn gpui_key_to_hotkey_token(key: &str) -> Option<String> {
    if let Some(token) = match key {
        "space" => Some("Space"),
        "enter" => Some("Enter"),
        "tab" => Some("Tab"),
        "backspace" => Some("Backspace"),
        "delete" => Some("Delete"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" => Some("PageUp"),
        "pagedown" => Some("PageDown"),
        "insert" => Some("Insert"),
        "up" => Some("ArrowUp"),
        "down" => Some("ArrowDown"),
        "left" => Some("ArrowLeft"),
        "right" => Some("ArrowRight"),
        "capslock" => Some("CapsLock"),
        "numlock" => Some("NumLock"),
        "scrolllock" => Some("ScrollLock"),
        "printscreen" => Some("PrintScreen"),
        "pause" => Some("Pause"),
        _ => None,
    } {
        return Some(token.to_string());
    }
    if key.len() == 1 {
        let byte = key.as_bytes()[0];
        if byte.is_ascii_alphabetic() {
            return Some((byte as char).to_ascii_uppercase().to_string());
        }
        if byte.is_ascii_digit() {
            return Some((byte as char).to_string());
        }
        return match byte {
            b'-' => Some("Minus"),
            b'=' => Some("Equal"),
            b'[' => Some("BracketLeft"),
            b']' => Some("BracketRight"),
            b'\\' => Some("Backslash"),
            b';' => Some("Semicolon"),
            b'\'' => Some("Quote"),
            b',' => Some("Comma"),
            b'.' => Some("Period"),
            b'/' => Some("Slash"),
            b'`' => Some("Backquote"),
            _ => None,
        }
        .map(String::from);
    }
    if let Some(digits) = key.strip_prefix('f') {
        if let Ok(number) = digits.parse::<u8>() {
            if (1..=24).contains(&number) {
                return Some(format!("F{number}"));
            }
        }
    }
    None
}

/// Human-readable global hotkey label for a settings field, e.g.
/// `Ctrl + Alt + Space`.
pub(crate) fn format_hotkey(hotkey: &HotKey) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(5);
    if hotkey.mods.contains(Modifiers::SUPER) {
        parts.push("Win".into());
    }
    if hotkey.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".into());
    }
    if hotkey.mods.contains(Modifiers::ALT) {
        parts.push("Alt".into());
    }
    if hotkey.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".into());
    }
    parts.push(code_label(hotkey.key));
    parts.join(" + ")
}

/// Short display name for a `Code`, stripping the `Key`/`Digit` prefixes and
/// reusing `keyboard-types`' Display for the rest.
fn code_label(code: Code) -> String {
    let text = code.to_string();
    if let Some(rest) = text.strip_prefix("Key") {
        return rest.to_string();
    }
    if let Some(rest) = text.strip_prefix("Digit") {
        return rest.to_string();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpui_key_token_mapping() {
        assert_eq!(gpui_key_to_hotkey_token("space").as_deref(), Some("Space"));
        assert_eq!(gpui_key_to_hotkey_token("enter").as_deref(), Some("Enter"));
        assert_eq!(gpui_key_to_hotkey_token("up").as_deref(), Some("ArrowUp"));
        assert_eq!(gpui_key_to_hotkey_token("f9").as_deref(), Some("F9"));
        assert_eq!(gpui_key_to_hotkey_token("a").as_deref(), Some("A"));
        assert_eq!(gpui_key_to_hotkey_token("9").as_deref(), Some("9"));
        assert_eq!(gpui_key_to_hotkey_token("-").as_deref(), Some("Minus"));
        assert_eq!(gpui_key_to_hotkey_token(";").as_deref(), Some("Semicolon"));
        // Modifier-only and otherwise unmappable keys never become a hotkey.
        assert_eq!(gpui_key_to_hotkey_token("control"), None);
        assert_eq!(gpui_key_to_hotkey_token("shift"), None);
        assert_eq!(gpui_key_to_hotkey_token("alt"), None);
        assert_eq!(gpui_key_to_hotkey_token("unidentified"), None);
    }

    #[test]
    fn keystroke_to_hotkey_requires_a_modifier() {
        let combo = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                alt: true,
                ..Default::default()
            },
            key: "space".into(),
            key_char: None,
        };
        let hotkey = keystroke_to_hotkey(&combo).expect("ctrl+alt+space maps to a hotkey");
        assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(hotkey.key, Code::Space);

        // A plain key must not become a global summon hotkey.
        let plain = gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: "space".into(),
            key_char: None,
        };
        assert!(keystroke_to_hotkey(&plain).is_none());

        // A modifier-only press has no main key to map.
        let modifier_only = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                ..Default::default()
            },
            key: "control".into(),
            key_char: None,
        };
        assert!(keystroke_to_hotkey(&modifier_only).is_none());
    }

    #[test]
    fn persisted_hotkey_string_roundtrips() {
        let recorded = keystroke_to_hotkey(&gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                alt: true,
                ..Default::default()
            },
            key: "space".into(),
            key_char: None,
        })
        .expect("valid combo");
        let persisted = recorded.to_string();
        assert_eq!(persisted, "control+alt+Space");
        assert_eq!(persisted.parse::<HotKey>().unwrap(), recorded);
    }

    #[test]
    fn format_hotkey_is_human_readable() {
        let default = HotkeyField::Summon.default_hotkey();
        assert_eq!(format_hotkey(&default), "Ctrl + Alt + Space");
        let settings = HotkeyField::Settings.default_hotkey();
        assert_eq!(format_hotkey(&settings), "Ctrl + Comma");
        assert_eq!(settings.to_string(), "control+Comma");
        assert_eq!(settings.to_string().parse::<HotKey>().unwrap(), settings);
        let combo = HotKey::new(Some(Modifiers::SHIFT | Modifiers::SUPER), Code::KeyA);
        assert_eq!(format_hotkey(&combo), "Win + Shift + A");
    }
}
