//! Global summon hotkey registration.

use std::cell::RefCell;
use std::rc::Rc;

use crate::config::{SETTINGS_HOTKEY_SETTING, SUMMON_HOTKEY_SETTING};
use crate::launcher_model::ActionId;
use crate::state::AppState;
use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};

pub(crate) const SUMMON_ACTION: &str = "steward.summon";
pub(crate) const SETTINGS_ACTION: &str = "steward.settings";

pub(crate) fn default_summon_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

pub(crate) fn default_settings_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL), Code::Comma)
}

pub(crate) fn persisted_summon_hotkey(storage: &steward_storage::Storage) -> HotKey {
    if let Some(hotkey) = std::env::var("STEWARD_SUMMON_HOTKEY")
        .ok()
        .and_then(|value| value.parse::<HotKey>().ok())
    {
        return hotkey;
    }
    storage
        .get_setting(SUMMON_HOTKEY_SETTING)
        .and_then(|value| value.parse::<HotKey>().ok())
        .unwrap_or_else(default_summon_hotkey)
}

pub(crate) fn persisted_settings_hotkey(storage: &steward_storage::Storage) -> HotKey {
    if let Some(hotkey) = std::env::var("STEWARD_SETTINGS_HOTKEY")
        .ok()
        .and_then(|value| value.parse::<HotKey>().ok())
    {
        return hotkey;
    }
    storage
        .get_setting(SETTINGS_HOTKEY_SETTING)
        .and_then(|value| value.parse::<HotKey>().ok())
        .unwrap_or_else(default_settings_hotkey)
}

pub(crate) fn setup(state: &Rc<RefCell<AppState>>) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let hotkey = persisted_summon_hotkey(&state.borrow().storage.borrow());
    if let Err(error) = manager.register(hotkey) {
        return Err(anyhow::anyhow!(
            "register summon hotkey {hotkey}: {error:#}"
        ));
    }
    let mut state = state.borrow_mut();
    if let Err(error) = state
        .hotkey_registry
        .register(ActionId::new(SUMMON_ACTION), &hotkey.to_string())
    {
        let _ = manager.unregister(hotkey);
        return Err(anyhow::anyhow!("register summon action: {error}"));
    }
    state.hotkey_manager = Some(manager);
    state.summon_hotkey = Some(hotkey);
    Ok(())
}

/// Replace the registered summon hotkey, restoring the previous binding if
/// the requested one cannot be registered.
pub(crate) fn replace_summon_hotkey(
    state: &Rc<RefCell<AppState>>,
    requested: &str,
) -> Result<HotKey> {
    let next = requested
        .parse::<HotKey>()
        .with_context(|| format!("parse summon hotkey '{requested}'"))?;
    let settings_hotkey = state.borrow().services.settings_hotkey();
    if settings_hotkey
        .parse::<HotKey>()
        .is_ok_and(|settings_hotkey| settings_hotkey == next)
    {
        anyhow::bail!("summon hotkey is already used by the settings shortcut");
    }
    let mut state = state.borrow_mut();
    let state = &mut *state;
    let AppState {
        hotkey_manager,
        hotkey_registry,
        summon_hotkey,
        ..
    } = state;
    let action = ActionId::new(SUMMON_ACTION);
    hotkey_registry
        .validate(&action, &next.to_string())
        .map_err(|error| anyhow::anyhow!("validate summon hotkey: {error}"))?;
    let manager = hotkey_manager
        .as_ref()
        .context("global hotkey manager is not initialized")?;
    let current = summon_hotkey.context("no summon hotkey is active")?;
    if current == next {
        return Ok(next);
    }
    manager
        .unregister(current)
        .context("unregister the previous summon hotkey")?;
    if let Err(error) = manager.register(next) {
        let restored = manager.register(current);
        return Err(anyhow::anyhow!(
            "register summon hotkey {next}: {error:#}; restore {current}: {}",
            restored
                .err()
                .map(|error| format!("{error:#}"))
                .unwrap_or_else(|| "ok".into())
        ));
    }
    if let Err(error) = hotkey_registry.register(action, &next.to_string()) {
        let _ = manager.unregister(next);
        let _ = manager.register(current);
        return Err(anyhow::anyhow!("register summon action {next}: {error}"));
    }
    *summon_hotkey = Some(next);
    Ok(next)
}

/// Validate and normalize the launcher-scoped settings shortcut. Unlike the
/// summon hotkey this is not registered system-wide, so it does not steal
/// Ctrl+, from the focused application when Steward is hidden.
pub(crate) fn replace_settings_hotkey(
    state: &Rc<RefCell<AppState>>,
    requested: &str,
) -> Result<HotKey> {
    let next = requested
        .parse::<HotKey>()
        .with_context(|| format!("parse settings hotkey '{requested}'"))?;
    let state = state.borrow();
    let summon_hotkey = state
        .summon_hotkey
        .or_else(|| state.services.summon_hotkey().parse::<HotKey>().ok());
    if summon_hotkey == Some(next) {
        anyhow::bail!("settings shortcut is already used by the summon hotkey");
    }
    Ok(next)
}

/// Register the settings shortcut only while the launcher is visible. The
/// registration is released as soon as the launcher hides, so the shortcut
/// remains launcher-scoped rather than becoming an app-wide hotkey.
pub(crate) fn register_launcher_settings_hotkey(state: &Rc<RefCell<AppState>>) -> Result<()> {
    let requested = state
        .borrow()
        .services
        .settings_hotkey()
        .parse::<HotKey>()
        .context("parse persisted settings hotkey")?;
    let mut state = state.borrow_mut();
    let state = &mut *state;
    let AppState {
        hotkey_manager,
        hotkey_registry,
        settings_hotkey,
        ..
    } = state;
    if *settings_hotkey == Some(requested) {
        return Ok(());
    }
    let manager = hotkey_manager
        .as_ref()
        .context("global hotkey manager is not initialized")?;
    let action = ActionId::new(SETTINGS_ACTION);
    hotkey_registry
        .validate(&action, &requested.to_string())
        .map_err(|error| anyhow::anyhow!("validate settings hotkey: {error}"))?;
    let previous = *settings_hotkey;
    if let Some(previous) = previous {
        manager
            .unregister(previous)
            .context("unregister the previous launcher settings hotkey")?;
    }
    if let Err(error) = manager.register(requested) {
        if let Some(previous) = previous {
            let _ = manager.register(previous);
        }
        return Err(anyhow::anyhow!(
            "register launcher settings hotkey {requested}: {error:#}"
        ));
    }
    if let Err(error) = hotkey_registry.register(action, &requested.to_string()) {
        let _ = manager.unregister(requested);
        if let Some(previous) = previous {
            let _ = manager.register(previous);
        }
        return Err(anyhow::anyhow!(
            "register launcher settings action {requested}: {error}"
        ));
    }
    *settings_hotkey = Some(requested);
    Ok(())
}

pub(crate) fn unregister_launcher_settings_hotkey(state: &Rc<RefCell<AppState>>) -> Result<()> {
    let mut state = state.borrow_mut();
    let Some(hotkey) = state.settings_hotkey.take() else {
        return Ok(());
    };
    let manager = state
        .hotkey_manager
        .as_ref()
        .context("global hotkey manager is not initialized")?;
    manager
        .unregister(hotkey)
        .context("unregister launcher settings hotkey")
}

pub(crate) fn launcher_shortcut(binding: &str) -> Option<String> {
    let hotkey = binding.parse::<HotKey>().ok()?;
    let code = hotkey.key.to_string();
    let key = match code.as_str() {
        "Backquote" => "`",
        "Backslash" => "\\",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Comma" => ",",
        "Equal" => "=",
        "Minus" => "-",
        "Period" => ".",
        "Quote" => "'",
        "Semicolon" => ";",
        "Slash" => "/",
        "Space" => "space",
        "Enter" => "enter",
        "Escape" => "escape",
        "Tab" => "tab",
        "Backspace" => "backspace",
        "Delete" => "delete",
        "Insert" => "insert",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Home" => "home",
        "End" => "end",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        _ => {
            let lowercase = code.to_ascii_lowercase();
            if let Some(letter) = lowercase.strip_prefix("key").filter(|key| key.len() == 1) {
                return modifier_prefix(hotkey).map(|prefix| format!("{prefix}{letter}"));
            }
            if let Some(digit) = lowercase.strip_prefix("digit").filter(|key| key.len() == 1) {
                return modifier_prefix(hotkey).map(|prefix| format!("{prefix}{digit}"));
            }
            if lowercase.strip_prefix('f').is_some_and(|number| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
            }) {
                return modifier_prefix(hotkey).map(|prefix| format!("{prefix}{lowercase}"));
            }
            return None;
        }
    };
    modifier_prefix(hotkey).map(|prefix| format!("{prefix}{key}"))
}

fn modifier_prefix(hotkey: HotKey) -> Option<String> {
    let mut prefix = String::new();
    if hotkey.mods.contains(Modifiers::SHIFT) {
        prefix.push_str("shift+");
    }
    if hotkey.mods.contains(Modifiers::CONTROL) {
        prefix.push_str("ctrl+");
    }
    if hotkey.mods.contains(Modifiers::ALT) {
        prefix.push_str("alt+");
    }
    if hotkey.mods.contains(Modifiers::SUPER) {
        prefix.push_str("super+");
    }
    (!prefix.is_empty() || hotkey.key.to_string().starts_with('F')).then_some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_hotkey_defaults_to_ctrl_comma() {
        assert_eq!(
            default_settings_hotkey().to_string(),
            "control+Comma".to_string()
        );
    }

    #[test]
    fn settings_hotkey_uses_the_launcher_keymap_syntax() {
        assert_eq!(
            launcher_shortcut("control+Comma").as_deref(),
            Some("ctrl+,")
        );
        assert_eq!(
            launcher_shortcut("control+alt+KeyP").as_deref(),
            Some("ctrl+alt+p")
        );
        assert!(launcher_shortcut("control+Numpad1").is_none());
    }
}
