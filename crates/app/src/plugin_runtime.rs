//! Plugin metadata, route, and launch coordination independent of launcher UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use steward_plugin_contract::{LaunchRequest, PluginErrorCode};
use steward_plugin_host::{HostConfig, PluginHost, PluginHostError, PluginLaunch};
use steward_plugin_registry::{PluginMeta, Registry};

const PLUGIN_RESCAN_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct PluginRuntimeState {
    host: PluginHost,
    registry: Registry,
    plugin_scan: Option<crossbeam_channel::Receiver<Vec<PluginMeta>>>,
    pending_launches: HashMap<String, PendingLaunch>,
    next_plugin_scan: Instant,
}

impl PluginRuntimeState {
    pub(crate) fn new(data_dir: &Path) -> anyhow::Result<Self> {
        let mut host = PluginHost::new(HostConfig::new(data_dir.to_path_buf()))?;
        let registry = Registry::open_with_root(
            data_dir,
            &std::env::var_os("STEWARD_PLUGINS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("plugins")),
        )?;
        let cached_plugins = registry.cached_plugins().unwrap_or_default();
        host.set_plugins(&cached_plugins);
        Ok(Self {
            host,
            registry,
            plugin_scan: None,
            pending_launches: HashMap::new(),
            next_plugin_scan: Instant::now(),
        })
    }

    pub(crate) fn start(&mut self) {
        self.spawn_plugin_scan();
    }

    pub(crate) fn host(&self) -> &PluginHost {
        &self.host
    }

    pub(crate) fn add_trust_root(&mut self, key: &str) -> anyhow::Result<()> {
        self.host.add_trust_root_base64(key)
    }

    pub(crate) fn start_launch(&mut self, request: LaunchRequest) -> Result<(), PluginHostError> {
        if let Some(pending) = self.pending_launches.get_mut(&request.plugin_id) {
            pending.latest = request;
            return Ok(());
        }
        let receiver = self.host.start_launch(request.clone())?;
        self.pending_launches.insert(
            request.plugin_id.clone(),
            PendingLaunch {
                requested_at: Instant::now(),
                latest: request.clone(),
                initial: request,
                receiver,
            },
        );
        Ok(())
    }

    pub(crate) fn accept_receipt(
        &mut self,
        receipt: &nsh_package::PackageReceipt,
    ) -> anyhow::Result<()> {
        self.host.accept_receipt(receipt)
    }

    pub(crate) fn poll_launch(&mut self) -> Option<PluginLaunchOutcome> {
        let mut finished = None;
        for (plugin_id, pending) in &self.pending_launches {
            let result = match pending.receiver.try_recv() {
                Ok(result) => result,
                Err(crossbeam_channel::TryRecvError::Empty) => continue,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Err(PluginHostError::WorkerExited)
                }
            };
            finished = Some((
                plugin_id.clone(),
                pending.requested_at,
                pending.initial.clone(),
                pending.latest.clone(),
                result,
            ));
            break;
        }
        let (plugin_id, requested_at, initial, latest, result) = finished?;
        self.pending_launches.remove(&plugin_id);
        Some(match result {
            Ok(launch) => PluginLaunchOutcome::Ready {
                requested_at,
                initial,
                latest,
                launch: Box::new(launch),
            },
            Err(error) => PluginLaunchOutcome::Failed {
                request: latest,
                code: error.code(),
                error: error.to_string(),
            },
        })
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        let plugin_scan = self.plugin_scan.clone();
        if let Some(rx) = plugin_scan {
            let mut finished = false;
            loop {
                match rx.try_recv() {
                    Ok(plugins) => {
                        changed |= self.host.set_plugins(&plugins);
                        finished = true;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                self.plugin_scan = None;
                self.next_plugin_scan = Instant::now() + PLUGIN_RESCAN_INTERVAL;
            }
        }
        if self.plugin_scan.is_none() && Instant::now() >= self.next_plugin_scan {
            self.spawn_plugin_scan();
        }
        changed
    }

    #[cfg(test)]
    pub(crate) fn insert_pending_launch_for_test(
        &mut self,
        plugin_id: &str,
        request: LaunchRequest,
        receiver: crossbeam_channel::Receiver<Result<PluginLaunch, PluginHostError>>,
    ) {
        self.pending_launches.insert(
            plugin_id.into(),
            PendingLaunch {
                requested_at: Instant::now(),
                initial: request.clone(),
                latest: request,
                receiver,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn pending_launch_command(&self, plugin_id: &str) -> Option<&str> {
        self.pending_launches
            .get(plugin_id)
            .map(|pending| pending.latest.command.as_str())
    }

    fn spawn_plugin_scan(&mut self) {
        if self.plugin_scan.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let data_dir = self.registry.data_dir().to_path_buf();
        let root = self.registry.plugins_root().to_path_buf();
        std::thread::spawn(move || {
            if let Ok(mut registry) = Registry::open_with_root(&data_dir, &root) {
                let _ = registry.scan();
                let _ = tx.send(registry.cached_plugins().unwrap_or_default());
            }
        });
        self.plugin_scan = Some(rx);
    }
}

struct PendingLaunch {
    requested_at: Instant,
    initial: LaunchRequest,
    latest: LaunchRequest,
    receiver: crossbeam_channel::Receiver<Result<PluginLaunch, PluginHostError>>,
}

pub(crate) enum PluginLaunchOutcome {
    Ready {
        requested_at: Instant,
        initial: LaunchRequest,
        latest: LaunchRequest,
        launch: Box<PluginLaunch>,
    },
    Failed {
        request: LaunchRequest,
        code: PluginErrorCode,
        error: String,
    },
}
