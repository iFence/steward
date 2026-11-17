//! Action-scoped hotkey registration with explicit conflict reporting.

use std::collections::{BTreeMap, HashMap};

use global_hotkey::hotkey::HotKey;

use crate::launcher_model::{ActionId, HotkeyBinding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HotkeyRegistrationError {
    InvalidBinding(String),
    AlreadyAssigned {
        action: ActionId,
        existing: ActionId,
        binding: String,
    },
}

impl std::fmt::Display for HotkeyRegistrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBinding(error) => formatter.write_str(error),
            Self::AlreadyAssigned {
                action,
                existing,
                binding,
            } => write!(
                formatter,
                "hotkey '{binding}' for action '{action}' is already assigned to '{existing}'"
            ),
        }
    }
}

impl std::error::Error for HotkeyRegistrationError {}

#[derive(Debug, Clone)]
struct RegisteredHotkey {
    binding: HotkeyBinding,
    hotkey: HotKey,
}

#[derive(Debug, Default)]
pub(crate) struct HotkeyRegistry {
    by_action: BTreeMap<ActionId, RegisteredHotkey>,
    by_id: HashMap<u32, ActionId>,
}

impl HotkeyRegistry {
    pub(crate) fn validate(
        &self,
        action: &ActionId,
        binding: &str,
    ) -> Result<HotKey, HotkeyRegistrationError> {
        let binding =
            HotkeyBinding::parse(binding).map_err(HotkeyRegistrationError::InvalidBinding)?;
        let hotkey = binding
            .as_str()
            .parse::<HotKey>()
            .map_err(|error| HotkeyRegistrationError::InvalidBinding(error.to_string()))?;
        if let Some(existing) = self.by_id.get(&hotkey.id()) {
            if existing != action {
                return Err(HotkeyRegistrationError::AlreadyAssigned {
                    action: action.clone(),
                    existing: existing.clone(),
                    binding: binding.as_str().to_owned(),
                });
            }
        }
        Ok(hotkey)
    }

    pub(crate) fn register(
        &mut self,
        action: ActionId,
        binding: &str,
    ) -> Result<HotKey, HotkeyRegistrationError> {
        let hotkey = self.validate(&action, binding)?;
        let canonical =
            HotkeyBinding::parse(binding).map_err(HotkeyRegistrationError::InvalidBinding)?;
        if let Some(previous) = self.by_action.get(&action) {
            self.by_id.remove(&previous.hotkey.id());
        }
        self.by_id.insert(hotkey.id(), action.clone());
        self.by_action.insert(
            action,
            RegisteredHotkey {
                binding: canonical,
                hotkey,
            },
        );
        Ok(hotkey)
    }

    pub(crate) fn unregister(&mut self, action: &ActionId) -> Option<HotKey> {
        let registered = self.by_action.remove(action)?;
        self.by_id.remove(&registered.hotkey.id());
        Some(registered.hotkey)
    }

    pub(crate) fn resolve(&self, id: u32) -> Option<&ActionId> {
        self.by_id.get(&id)
    }

    pub(crate) fn binding(&self, action: &ActionId) -> Option<&str> {
        self.by_action
            .get(action)
            .map(|registered| registered.binding.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_a_conflicting_hotkey_returns_the_existing_action() {
        let mut registry = HotkeyRegistry::default();
        let first = ActionId::new("steward.summon");
        let second = ActionId::new("plugin:test:run");
        registry
            .register(first.clone(), "control+alt+Space")
            .unwrap();

        let error = registry.register(second, "alt+control+Space").unwrap_err();
        assert!(matches!(
            error,
            HotkeyRegistrationError::AlreadyAssigned { existing, .. }
                if existing == first
        ));
        assert_eq!(
            registry.resolve(registry.by_action[&first].hotkey.id()),
            Some(&first)
        );
    }

    #[test]
    fn replacing_an_actions_binding_releases_the_previous_id() {
        let mut registry = HotkeyRegistry::default();
        let action = ActionId::new("steward.summon");
        let old = registry
            .register(action.clone(), "control+alt+Space")
            .unwrap();
        let new = registry
            .register(action.clone(), "control+alt+Comma")
            .unwrap();

        assert!(registry.resolve(old.id()).is_none());
        assert_eq!(registry.resolve(new.id()), Some(&action));
        assert_eq!(registry.binding(&action), Some("control+alt+Comma"));
    }
}
