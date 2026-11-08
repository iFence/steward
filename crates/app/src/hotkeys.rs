//! Global summon hotkey registration.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};

use crate::config::SUMMON_HOTKEY_SETTING;
use crate::state::AppState;

pub(crate) fn default_summon_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

pub(crate) fn persisted_summon_hotkey(storage: &steward_storage::Storage) -> HotKey {
    storage
        .get_setting(SUMMON_HOTKEY_SETTING)
        .and_then(|value| value.parse::<HotKey>().ok())
        .unwrap_or_else(default_summon_hotkey)
}

pub(crate) fn setup(state: &Rc<RefCell<AppState>>) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let mut hotkey = persisted_summon_hotkey(&state.borrow().storage.borrow());
    if let Err(error) = manager.register(hotkey) {
        eprintln!("failed to register summon hotkey {hotkey}: {error:#}; falling back to default");
        hotkey = default_summon_hotkey();
        manager
            .register(hotkey)
            .context("register default summon hotkey")?;
    }
    let mut state = state.borrow_mut();
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
    let manager = state
        .hotkey_manager
        .as_ref()
        .context("global hotkey manager is not initialized")?;
    let current = state.summon_hotkey.context("no summon hotkey is active")?;
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
    state.summon_hotkey = Some(next);
    Ok(next)
}
