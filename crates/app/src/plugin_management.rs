//! Background bridge between the settings surface and the local plugin store.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};

use steward_plugin_store::{MarketplaceIndex, PluginCatalog, PluginOperationKind, PluginStore};

#[derive(Debug, Clone)]
pub(crate) struct PluginManagementSnapshot {
    pub(crate) busy: bool,
    pub(crate) catalog: PluginCatalog,
    pub(crate) message: Option<String>,
    pub(crate) error: Option<String>,
}

impl Default for PluginManagementSnapshot {
    fn default() -> Self {
        Self {
            busy: false,
            catalog: PluginCatalog {
                installed: Vec::new(),
                marketplace: MarketplaceIndex::default(),
                marketplace_errors: Vec::new(),
                revocations: None,
            },
            message: None,
            error: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PluginManagementPort {
    state: Arc<RwLock<PluginManagementSnapshot>>,
    revision: Arc<AtomicU64>,
    commands: Option<crossbeam_channel::Sender<Command>>,
}

impl Default for PluginManagementPort {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(PluginManagementSnapshot::default())),
            revision: Arc::new(AtomicU64::new(0)),
            commands: None,
        }
    }
}

impl PluginManagementPort {
    pub(crate) fn spawn(store: PluginStore) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let port = Self {
            state: Arc::new(RwLock::new(PluginManagementSnapshot::default())),
            revision: Arc::new(AtomicU64::new(0)),
            commands: Some(tx.clone()),
        };
        let state = port.state.clone();
        let revision = port.revision.clone();
        std::thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                set_busy(&state, &revision, true, None, None);
                let outcome = match command {
                    Command::Refresh => Ok(None),
                    Command::SyncMarketplace => store.sync_marketplace().map(|report| {
                        Some(format!("Indexed {} plugin package(s).", report.entries))
                    }),
                    Command::Install(plugin_id) => store
                        .install_from_marketplace(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Update(plugin_id) => store
                        .update_from_marketplace(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Remove(plugin_id) => store
                        .remove(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Rollback(plugin_id) => store
                        .rollback(&plugin_id, None)
                        .map(|operation| Some(operation_message(operation))),
                };
                let (message, error) = match outcome {
                    Ok(message) => (message, None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                let catalog = store.catalog();
                if let Ok(mut current) = state.write() {
                    *current = PluginManagementSnapshot {
                        busy: false,
                        catalog,
                        message,
                        error,
                    };
                }
                revision.fetch_add(1, Ordering::Release);
            }
        });
        port
    }

    pub(crate) fn snapshot(&self) -> PluginManagementSnapshot {
        self.state
            .read()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn refresh(&self) {
        self.send(Command::Refresh);
    }

    pub(crate) fn sync_marketplace(&self) {
        self.send(Command::SyncMarketplace);
    }

    pub(crate) fn install(&self, plugin_id: impl Into<String>) {
        self.send(Command::Install(plugin_id.into()));
    }

    pub(crate) fn update(&self, plugin_id: impl Into<String>) {
        self.send(Command::Update(plugin_id.into()));
    }

    pub(crate) fn remove(&self, plugin_id: impl Into<String>) {
        self.send(Command::Remove(plugin_id.into()));
    }

    pub(crate) fn rollback(&self, plugin_id: impl Into<String>) {
        self.send(Command::Rollback(plugin_id.into()));
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

enum Command {
    Refresh,
    SyncMarketplace,
    Install(String),
    Update(String),
    Remove(String),
    Rollback(String),
}

fn operation_message(operation: steward_plugin_store::PluginOperation) -> String {
    let verb = match operation.action {
        PluginOperationKind::Install => "Installed",
        PluginOperationKind::Update => "Updated",
        PluginOperationKind::Remove => "Removed",
        PluginOperationKind::Rollback => "Rolled back",
    };
    format!("{verb} {} {}.", operation.name, operation.version)
}

fn set_busy(
    state: &Arc<RwLock<PluginManagementSnapshot>>,
    revision: &Arc<AtomicU64>,
    busy: bool,
    message: Option<String>,
    error: Option<String>,
) {
    if let Ok(mut current) = state.write() {
        current.busy = busy;
        current.message = message;
        current.error = error;
    }
    revision.fetch_add(1, Ordering::Release);
}
