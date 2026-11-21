//! Host-owned system opening and installed-application discovery.
//!
//! Plugins can name a path and an application from Steward's installed-app
//! catalog, but cannot provide a command line. Paths are authorized against
//! user-selected filesystem roots before any system call.

use std::path::{Path, PathBuf};

use nsh_engine::HostReply;
use steward_core_engine::{platform_scanner, AppEntry};

use crate::plugin_filesystem::{FileAccess, FileError};

const SYSTEM_WORKERS: usize = 2;
const SYSTEM_QUEUE_DEPTH: usize = 8;

enum SystemTask {
    OpenPath {
        path: String,
        reply: HostReply,
    },
    RevealPath {
        path: String,
        reply: HostReply,
    },
    ListApplications {
        reply: HostReply,
    },
    OpenWith {
        application_id: String,
        path: String,
        reply: HostReply,
    },
}

#[derive(Clone)]
pub(crate) struct SystemService {
    tasks: crossbeam_channel::Sender<SystemTask>,
}

impl SystemService {
    pub(crate) fn new(access: FileAccess) -> Self {
        let (tasks, receiver) = crossbeam_channel::bounded::<SystemTask>(SYSTEM_QUEUE_DEPTH);
        for worker in 0..SYSTEM_WORKERS {
            let receiver = receiver.clone();
            let access = access.clone();
            std::thread::Builder::new()
                .name(format!("steward-plugin-system-{worker}"))
                .spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        match task {
                            SystemTask::OpenPath { path, reply } => audit_path_result(
                                &access,
                                "steward.system.openPath",
                                &path,
                                open_path(&access, &path),
                                reply,
                            ),
                            SystemTask::RevealPath { path, reply } => audit_path_result(
                                &access,
                                "steward.system.revealPath",
                                &path,
                                reveal_path(&access, &path),
                                reply,
                            ),
                            SystemTask::ListApplications { reply } => match list_applications() {
                                Ok(applications) => {
                                    access.audit(
                                        "steward.system.applications",
                                        "system=installed",
                                        "allowed",
                                        &format!("applications={}", applications.len()),
                                    );
                                    reply.resolve(
                                        serde_json::to_value(applications).unwrap_or_default(),
                                    );
                                }
                                Err(error) => reply.reject(error),
                            },
                            SystemTask::OpenWith {
                                application_id,
                                path,
                                reply,
                            } => {
                                let result = open_with(&access, &application_id, &path);
                                let outcome = if result.is_ok() { "allowed" } else { "denied" };
                                access.audit(
                                    "steward.system.openWith",
                                    &format!("application={application_id};path={path}"),
                                    outcome,
                                    "origin=host_call",
                                );
                                match result {
                                    Ok(()) => reply.resolve(serde_json::json!({})),
                                    Err(error) => reply.reject(error),
                                }
                            }
                        }
                    }
                })
                .expect("spawn plugin system worker");
        }
        Self { tasks }
    }

    pub(crate) fn enqueue_open_path(&self, path: String, reply: HostReply) {
        self.enqueue(SystemTask::OpenPath { path, reply });
    }

    pub(crate) fn enqueue_reveal_path(&self, path: String, reply: HostReply) {
        self.enqueue(SystemTask::RevealPath { path, reply });
    }

    pub(crate) fn enqueue_list_applications(&self, reply: HostReply) {
        self.enqueue(SystemTask::ListApplications { reply });
    }

    pub(crate) fn enqueue_open_with(&self, application_id: String, path: String, reply: HostReply) {
        self.enqueue(SystemTask::OpenWith {
            application_id,
            path,
            reply,
        });
    }

    fn enqueue(&self, task: SystemTask) {
        let task = match self.tasks.try_send(task) {
            Ok(()) => return,
            Err(crossbeam_channel::TrySendError::Full(task))
            | Err(crossbeam_channel::TrySendError::Disconnected(task)) => task,
        };
        match task {
            SystemTask::OpenPath { path: _, reply }
            | SystemTask::RevealPath { path: _, reply }
            | SystemTask::ListApplications { reply }
            | SystemTask::OpenWith {
                application_id: _,
                path: _,
                reply,
            } => reply
                .reject("[resource_exhausted] the system capability queue is full; retry later"),
        }
    }
}

fn open_path(access: &FileAccess, path: &str) -> Result<PathBuf, FileError> {
    let path = access.authorize_read(path)?;
    crate::launch::open_file(&path)
        .map_err(|error| FileError::IoFailure(format!("open path: {error:#}")))?;
    Ok(path)
}

fn reveal_path(access: &FileAccess, path: &str) -> Result<PathBuf, FileError> {
    let path = access.authorize_read(path)?;
    crate::launch::reveal_in_explorer(&path)
        .map_err(|error| FileError::IoFailure(format!("reveal path: {error:#}")))?;
    Ok(path)
}

fn list_applications() -> Result<Vec<ApplicationEntry>, String> {
    let mut applications = platform_scanner()
        .scan()
        .into_iter()
        .filter_map(application_entry)
        .collect::<Vec<_>>();
    applications.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    applications.dedup_by(|left, right| left.id == right.id);
    if applications.len() > 2_000 {
        applications.truncate(2_000);
    }
    Ok(applications)
}

fn application_entry(application: AppEntry) -> Option<ApplicationEntry> {
    let id = application.path.to_string_lossy().into_owned();
    if id.is_empty() || (!application.path.is_absolute() && !id.starts_with("shell:AppsFolder\\")) {
        return None;
    }
    Some(ApplicationEntry {
        id,
        name: application.name,
    })
}

fn open_with(access: &FileAccess, application_id: &str, value: &str) -> Result<(), String> {
    let requested = access
        .authorize_read(value)
        .map_err(|error| error.host_message())?;
    let applications = list_applications()?;
    let application = applications
        .iter()
        .find(|application| application.id == application_id)
        .ok_or_else(|| {
            "[permission_denied] application is not in Steward's installed-app catalog".to_owned()
        })?;
    crate::launch::launch_with_resource(Path::new(&application.id), &requested)
        .map_err(|error| format!("[io_failure] open with application: {error:#}"))
}

fn audit_path_result(
    access: &FileAccess,
    capability: &str,
    path: &str,
    result: Result<PathBuf, FileError>,
    reply: HostReply,
) {
    match result {
        Ok(resolved) => {
            access.audit(
                capability,
                &format!("path={}", resolved.display()),
                "allowed",
                "origin=host_call",
            );
            reply.resolve(serde_json::json!({ "path": resolved }));
        }
        Err(error) => {
            access.audit(
                capability,
                &format!("path={path}"),
                if matches!(error, FileError::PermissionDenied(_)) {
                    "denied"
                } else {
                    "failed"
                },
                &error.host_message(),
            );
            reply.reject(error.host_message());
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationEntry {
    id: String,
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_entries_allow_files_and_shell_application_aliases() {
        let application = ApplicationEntry {
            id: r"C:\Program Files\Editor\editor.exe".into(),
            name: "Editor".into(),
        };
        assert!(Path::new(&application.id).is_absolute());
        assert!(!application.id.contains(" --"));
        assert!(application_entry(AppEntry {
            name: "Calculator".into(),
            path: PathBuf::from(r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
        })
        .is_some());
    }

    #[test]
    fn open_with_rejects_an_unknown_application_before_launch() {
        let access = FileAccess {
            data_dir: std::env::temp_dir().join("steward-system-test"),
            plugin_id: "com.example.test".into(),
            publisher_fingerprint: "SHA256:publisher".into(),
            version: "1.0.0".into(),
        };
        let error =
            open_with(&access, "not-an-installed-application", r"C:\missing.txt").unwrap_err();
        assert!(error.contains("permission_denied") || error.contains("io_failure"));
    }
}
