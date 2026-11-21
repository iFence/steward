//! Global summon hotkey registration.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};

use crate::config::SUMMON_HOTKEY_SETTING;
use crate::launcher_model::ActionId;
use crate::state::AppState;

pub(crate) const SUMMON_ACTION: &str = "steward.summon";

pub(crate) fn default_summon_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
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
