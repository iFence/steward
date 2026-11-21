//! Scoped filesystem host calls for isolated plugins.
//!
//! A path grant is never inferred from a plugin-declared root. The user picks
//! a file or directory through Steward's native dialog, and that exact
//! canonical path is stored in the capability grant table. Every later call
//! re-reads the active grants, canonicalizes the requested path, and rejects
//! paths that resolve outside the approved roots.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nsh_engine::HostReply;
use steward_plugin_contract::capabilities::{FS_READ, FS_WRITE};
use steward_storage::{NewCapabilityAuditEntry, NewCapabilityGrant, Storage};

const FILE_WORKERS: usize = 2;
const FILE_QUEUE_DEPTH: usize = 16;
const MAX_WALK_TIMEOUT_MS: u64 = 30_000;
const MAX_WALK_RESULTS: u64 = 20_000;
const MAX_WALK_DEPTH: u64 = 64;
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 32_768;

#[derive(Debug, Clone)]
pub(crate) struct FileAccess {
    pub(crate) data_dir: PathBuf,
    pub(crate) plugin_id: String,
    pub(crate) publisher_fingerprint: String,
    pub(crate) version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerMode {
    FolderRead,
    FolderWrite,
    FileRead,
}

impl PickerMode {
    pub(crate) fn from_name(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("folderRead") {
            "folderRead" | "readFolder" => Some(Self::FolderRead),
            "folderWrite" | "writeFolder" | "readWriteFolder" => Some(Self::FolderWrite),
            "fileRead" | "readFile" => Some(Self::FileRead),
            _ => None,
        }
    }

    pub(crate) fn selects_files(self) -> bool {
        self == Self::FileRead
    }

    pub(crate) fn grants_write(self) -> bool {
        self == Self::FolderWrite
    }
}

#[derive(Debug)]
pub(crate) enum FileError {
    PermissionDenied(String),
    InvalidInput(String),
    NotFound(String),
    ResourceExhausted(String),
    Timeout(String),
    Cancelled(String),
    HostFailure(String),
    IoFailure(String),
}

impl FileError {
    pub(crate) fn host_message(&self) -> String {
        let (code, message) = match self {
            Self::PermissionDenied(message) => ("permission_denied", message),
            Self::InvalidInput(message) => ("invalid_input", message),
            Self::NotFound(message) => ("io_failure", message),
            Self::ResourceExhausted(message) => ("resource_exhausted", message),
            Self::Timeout(message) => ("timeout", message),
            Self::Cancelled(message) => ("cancelled", message),
            Self::HostFailure(message) => ("host_failure", message),
            Self::IoFailure(message) => ("io_failure", message),
        };
        format!("[{code}] {message}")
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.host_message())
    }
}

impl std::error::Error for FileError {}

type FileResult<T> = Result<T, FileError>;

struct WalkRequest {
    id: String,
    root: String,
    max_results: u64,
    max_depth: u64,
    timeout: Duration,
    reply: HostReply,
}

struct ReadRequest {
    path: String,
    max_bytes: u64,
    reply: HostReply,
}

struct WriteRequest {
    path: String,
    contents: String,
    reply: HostReply,
}

struct GrantsRequest {
    reply: HostReply,
}

enum FileTask {
    Walk(WalkRequest),
    Read(ReadRequest),
    Write(WriteRequest),
    Grants(GrantsRequest),
}

#[derive(Clone)]
pub(crate) struct FileService {
    access: FileAccess,
    tasks: crossbeam_channel::Sender<FileTask>,
    cancelled_walks: Arc<Mutex<BTreeSet<String>>>,
}

impl FileService {
    pub(crate) fn new(access: FileAccess) -> Self {
        let (tasks, receiver) = crossbeam_channel::bounded::<FileTask>(FILE_QUEUE_DEPTH);
        let cancelled_walks = Arc::new(Mutex::new(BTreeSet::new()));
        for worker in 0..FILE_WORKERS {
            let receiver = receiver.clone();
            let access = access.clone();
            let cancelled_walks = cancelled_walks.clone();
            std::thread::Builder::new()
                .name(format!("steward-plugin-fs-{worker}"))
                .spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        match task {
                            FileTask::Walk(request) => {
                                complete_walk(&access, &cancelled_walks, request)
                            }
                            FileTask::Read(request) => complete_read(&access, request),
                            FileTask::Write(request) => complete_write(&access, request),
                            FileTask::Grants(request) => complete_grants(&access, request),
                        }
                    }
                })
                .expect("spawn plugin filesystem worker");
        }
        Self {
            access,
            tasks,
            cancelled_walks,
        }
    }

    pub(crate) fn picker_grant(&self, selected: &Path, mode: PickerMode) -> FileResult<PathBuf> {
        let canonical = canonical_existing(selected, "selected path")?;
        if mode != PickerMode::FolderWrite && mode != PickerMode::FolderRead && !canonical.is_file()
        {
            return Err(FileError::InvalidInput(
                "the selected file is not a regular file".into(),
            ));
        }
        if mode != PickerMode::FileRead && !canonical.is_dir() {
            return Err(FileError::InvalidInput(
                "the selected folder is not a directory".into(),
            ));
        }
        let grant = |capability: &str| {
            self.grant_path(capability, &canonical).map_err(|error| {
                FileError::HostFailure(format!("remember selected path: {error:#}"))
            })
        };
        grant(FS_READ)?;
        if mode.grants_write() {
            grant(FS_WRITE)?;
        }
        self.access.audit(
            if mode.grants_write() {
                FS_WRITE
            } else {
                FS_READ
            },
            &format!("path={}", display_path(&canonical)),
            "granted",
            "origin=picker",
        );
        Ok(canonical)
    }

    pub(crate) fn enqueue_walk(
        &self,
        id: String,
        root: String,
        max_results: u64,
        max_depth: u64,
        timeout_ms: u64,
        reply: HostReply,
    ) {
        if !valid_walk_id(&id) {
            reply.reject("[invalid_input] walk id must be 1 to 128 safe characters");
            return;
        }
        if max_results == 0 || max_results > MAX_WALK_RESULTS {
            reply.reject(format!(
                "[invalid_input] maxResults must be between 1 and {MAX_WALK_RESULTS}"
            ));
            return;
        }
        if max_depth == 0 || max_depth > MAX_WALK_DEPTH {
            reply.reject(format!(
                "[invalid_input] maxDepth must be between 1 and {MAX_WALK_DEPTH}"
            ));
            return;
        }
        if timeout_ms == 0 || timeout_ms > MAX_WALK_TIMEOUT_MS {
            reply.reject(format!(
                "[invalid_input] timeoutMs must be between 1 and {MAX_WALK_TIMEOUT_MS}"
            ));
            return;
        }
        if let Ok(mut cancelled) = self.cancelled_walks.lock() {
            cancelled.remove(&id);
        }
        self.enqueue(FileTask::Walk(WalkRequest {
            id,
            root,
            max_results,
            max_depth,
            timeout: Duration::from_millis(timeout_ms),
            reply,
        }));
    }

    pub(crate) fn cancel_walk(&self, id: &str) -> bool {
        if !valid_walk_id(id) {
            return false;
        }
        self.cancelled_walks
            .lock()
            .map(|mut cancelled| {
                if cancelled.len() >= 1_024 && !cancelled.contains(id) {
                    return false;
                }
                cancelled.insert(id.to_owned())
            })
            .unwrap_or(false)
    }

    pub(crate) fn enqueue_read(&self, path: String, max_bytes: u64, reply: HostReply) {
        if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
            reply.reject(format!(
                "[invalid_input] maxBytes must be between 1 and {MAX_READ_BYTES}"
            ));
            return;
        }
        self.enqueue(FileTask::Read(ReadRequest {
            path,
            max_bytes,
            reply,
        }));
    }

    pub(crate) fn enqueue_write(&self, path: String, contents: String, reply: HostReply) {
        if contents.len() > MAX_WRITE_BYTES {
            reply.reject(format!(
                "[resource_exhausted] text content exceeds {MAX_WRITE_BYTES} bytes"
            ));
            return;
        }
        self.enqueue(FileTask::Write(WriteRequest {
            path,
            contents,
            reply,
        }));
    }

    pub(crate) fn enqueue_grants(&self, reply: HostReply) {
        self.enqueue(FileTask::Grants(GrantsRequest { reply }));
    }

    fn enqueue(&self, task: FileTask) {
        let task = match self.tasks.try_send(task) {
            Ok(()) => return,
            Err(crossbeam_channel::TrySendError::Full(task))
            | Err(crossbeam_channel::TrySendError::Disconnected(task)) => task,
        };
        match task {
            FileTask::Walk(request) => request
                .reply
                .reject("[resource_exhausted] the filesystem walk queue is full; retry later"),
            FileTask::Read(request) => request
                .reply
                .reject("[resource_exhausted] the filesystem read queue is full; retry later"),
            FileTask::Write(request) => request
                .reply
                .reject("[resource_exhausted] the filesystem write queue is full; retry later"),
            FileTask::Grants(request) => request
                .reply
                .reject("[resource_exhausted] the filesystem grants queue is full; retry later"),
        }
    }

    fn grant_path(&self, capability: &str, path: &Path) -> anyhow::Result<()> {
        let storage = Storage::open_at(&self.access.data_dir)?;
        storage.grant_capability(&NewCapabilityGrant {
            plugin_id: self.access.plugin_id.clone(),
            publisher_fingerprint: self.access.publisher_fingerprint.clone(),
            version_range: format!("^{}", self.access.version),
            capability: capability.into(),
            scope: format!("path={}", display_path(path)),
            granted_at: unix_seconds(),
        })?;
        Ok(())
    }
}

impl FileAccess {
    fn matching_paths(&self, capability: &str) -> FileResult<Vec<PathBuf>> {
        let storage = Storage::open_at(&self.data_dir)
            .map_err(|error| FileError::HostFailure(format!("open capability store: {error:#}")))?;
        let grants = storage
            .matching_capability_grants(&self.plugin_id, &self.publisher_fingerprint, &self.version)
            .map_err(|error| {
                FileError::HostFailure(format!("read active path grants: {error:#}"))
            })?;
        let mut paths = grants
            .into_iter()
            .filter(|grant| grant.capability == capability)
            .filter_map(|grant| grant.scope.strip_prefix("path=").map(PathBuf::from))
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| path_key(path));
        paths.dedup_by(|left, right| path_key(left) == path_key(right));
        Ok(paths)
    }

    pub(crate) fn audit(&self, capability: &str, scope: &str, outcome: &str, detail: &str) {
        let Ok(storage) = Storage::open_at(&self.data_dir) else {
            return;
        };
        let _ = storage.append_capability_audit(&NewCapabilityAuditEntry {
            plugin_id: self.plugin_id.clone(),
            publisher_fingerprint: self.publisher_fingerprint.clone(),
            capability: capability.into(),
            scope: scope.into(),
            outcome: outcome.into(),
            occurred_at: unix_seconds(),
            detail: Some(format!(
                "origin=host_call;version={};{detail}",
                self.version
            )),
        });
    }
}

fn complete_walk(
    access: &FileAccess,
    cancelled_walks: &Arc<Mutex<BTreeSet<String>>>,
    request: WalkRequest,
) {
    let result = access.walk(
        cancelled_walks,
        &request.id,
        &request.root,
        request.max_results as usize,
        request.max_depth as usize,
        request.timeout,
    );
    if let Ok(mut cancelled) = cancelled_walks.lock() {
        cancelled.remove(&request.id);
    }
    match result {
        Ok(result) => {
            access.audit(
                "steward.fs.walk",
                &format!("path={}", display_path(&result.root)),
                "allowed",
                &format!(
                    "walk_id={};entries={};truncated={}",
                    request.id,
                    result.entries.len(),
                    result.truncated
                ),
            );
            request
                .reply
                .resolve(serde_json::to_value(result).unwrap_or_default());
        }
        Err(error) => {
            access.audit(
                "steward.fs.walk",
                &format!("path={}", request.root),
                match error {
                    FileError::PermissionDenied(_) => "denied",
                    FileError::Cancelled(_) => "cancelled",
                    _ => "failed",
                },
                &format!("walk_id={};error={}", request.id, error.host_message()),
            );
            request.reply.reject(error.host_message());
        }
    }
}

fn complete_read(access: &FileAccess, request: ReadRequest) {
    match access.read_text(&request.path, request.max_bytes as usize) {
        Ok(result) => {
            access.audit(
                "steward.fs.readText",
                &format!("path={}", request.path),
                "allowed",
                &format!("bytes={}", result.len()),
            );
            request
                .reply
                .resolve(serde_json::to_value(result).unwrap_or_default());
        }
        Err(error) => {
            access.audit(
                "steward.fs.readText",
                &format!("path={}", request.path),
                if matches!(error, FileError::PermissionDenied(_)) {
                    "denied"
                } else {
                    "failed"
                },
                &format!("error={}", error.host_message()),
            );
            request.reply.reject(error.host_message());
        }
    }
}

fn complete_write(access: &FileAccess, request: WriteRequest) {
    match access.write_text(&request.path, &request.contents) {
        Ok(path) => {
            access.audit(
                "steward.fs.writeText",
                &format!("path={}", request.path),
                "allowed",
                &format!(
                    "bytes={};resolved={}",
                    request.contents.len(),
                    path.display()
                ),
            );
            request
                .reply
                .resolve(serde_json::json!({ "path": display_path(&path) }));
        }
        Err(error) => {
            access.audit(
                "steward.fs.writeText",
                &format!("path={}", request.path),
                if matches!(error, FileError::PermissionDenied(_)) {
                    "denied"
                } else {
                    "failed"
                },
                &format!("error={}", error.host_message()),
            );
            request.reply.reject(error.host_message());
        }
    }
}

fn complete_grants(access: &FileAccess, request: GrantsRequest) {
    match access.list_grants() {
        Ok(grants) => request
            .reply
            .resolve(serde_json::to_value(grants).unwrap_or_default()),
        Err(error) => request.reply.reject(error.host_message()),
    }
}

impl FileAccess {
    pub(crate) fn authorize_read(&self, value: &str) -> FileResult<PathBuf> {
        let roots = self.matching_paths(FS_READ)?;
        self.resolve_existing(value, &roots, "read")
    }

    pub(crate) fn authorize_write_directory(&self, value: &str) -> FileResult<PathBuf> {
        let roots = self.matching_paths(FS_WRITE)?;
        let path = self.resolve_existing(value, &roots, "write")?;
        if !path.is_dir() {
            return Err(FileError::InvalidInput(
                "controlled process working directory must be a directory".into(),
            ));
        }
        Ok(path)
    }

    fn read_text(&self, value: &str, max_bytes: usize) -> FileResult<String> {
        let path = self.authorize_read(value)?;
        let metadata = std::fs::metadata(&path)
            .map_err(|error| FileError::IoFailure(format!("inspect file: {error}")))?;
        if !metadata.is_file() {
            return Err(FileError::InvalidInput(
                "readText requires a regular file".into(),
            ));
        }
        if metadata.len() > max_bytes as u64 {
            return Err(FileError::ResourceExhausted(format!(
                "file exceeds the {max_bytes} byte read limit"
            )));
        }
        let file = std::fs::File::open(&path)
            .map_err(|error| FileError::IoFailure(format!("open file: {error}")))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| FileError::IoFailure(format!("read file: {error}")))?;
        if bytes.len() > max_bytes {
            return Err(FileError::ResourceExhausted(format!(
                "file exceeds the {max_bytes} byte read limit"
            )));
        }
        String::from_utf8(bytes)
            .map_err(|_| FileError::InvalidInput("file is not valid UTF-8 text".into()))
    }

    fn write_text(&self, value: &str, contents: &str) -> FileResult<PathBuf> {
        let roots = self.matching_paths(FS_WRITE)?;
        let path = self.resolve_for_write(value, &roots)?;
        std::fs::write(&path, contents)
            .map_err(|error| FileError::IoFailure(format!("write file: {error}")))?;
        Ok(path)
    }

    fn list_grants(&self) -> FileResult<serde_json::Value> {
        let read = self
            .matching_paths(FS_READ)?
            .into_iter()
            .map(|path| (path_key(&path), display_path(&path)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let write = self
            .matching_paths(FS_WRITE)?
            .into_iter()
            .map(|path| (path_key(&path), display_path(&path)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut grants = std::collections::BTreeMap::<String, serde_json::Value>::new();
        for (key, path) in &read {
            grants.insert(
                key.clone(),
                serde_json::json!({ "path": path, "read": true, "write": false }),
            );
        }
        for (key, path) in &write {
            grants.insert(
                key.clone(),
                serde_json::json!({ "path": path, "read": read.contains_key(key), "write": true }),
            );
        }
        Ok(serde_json::Value::Array(grants.into_values().collect()))
    }

    fn resolve_existing(
        &self,
        value: &str,
        roots: &[PathBuf],
        operation: &str,
    ) -> FileResult<PathBuf> {
        validate_path_input(value)?;
        if roots.is_empty() {
            return Err(FileError::PermissionDenied(format!(
                "no user-selected root grants {operation} access"
            )));
        }
        let path = canonical_existing(Path::new(value), operation)?;
        if !roots.iter().any(|root| within(&path, root)) {
            return Err(FileError::PermissionDenied(format!(
                "path is outside the plugin's {operation} grants"
            )));
        }
        Ok(path)
    }

    fn resolve_for_write(&self, value: &str, roots: &[PathBuf]) -> FileResult<PathBuf> {
        validate_path_input(value)?;
        if roots.is_empty() {
            return Err(FileError::PermissionDenied(
                "no user-selected root grants write access".into(),
            ));
        }
        let requested = Path::new(value);
        let parent = requested
            .parent()
            .ok_or_else(|| FileError::InvalidInput("path has no parent directory".into()))?;
        let canonical_parent = canonical_existing(parent, "parent directory")?;
        if !roots.iter().any(|root| within(&canonical_parent, root)) {
            return Err(FileError::PermissionDenied(
                "path is outside the plugin's write grants".into(),
            ));
        }
        let name = requested
            .file_name()
            .ok_or_else(|| FileError::InvalidInput("path has no file name".into()))?;
        let target = canonical_parent.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if is_reparse_point(&metadata) {
                return Err(FileError::PermissionDenied(
                    "refusing to write through a symbolic link or reparse point".into(),
                ));
            }
            let canonical = std::fs::canonicalize(&target)
                .map_err(|error| FileError::IoFailure(format!("resolve file: {error}")))?;
            if !roots.iter().any(|root| within(&canonical, root)) {
                return Err(FileError::PermissionDenied(
                    "path resolves outside the plugin's write grants".into(),
                ));
            }
            if metadata.is_dir() {
                return Err(FileError::InvalidInput(
                    "writeText requires a file path".into(),
                ));
            }
            return Ok(canonical);
        }
        Ok(target)
    }

    fn walk(
        &self,
        cancelled_walks: &Arc<Mutex<BTreeSet<String>>>,
        id: &str,
        root: &str,
        max_results: usize,
        max_depth: usize,
        timeout: Duration,
    ) -> FileResult<WalkResult> {
        let roots = self.matching_paths(FS_READ)?;
        let root = self.resolve_existing(root, &roots, "read")?;
        if !root.is_dir() {
            return Err(FileError::InvalidInput(
                "walk root must be a directory".into(),
            ));
        }
        let started = Instant::now();
        let mut entries = Vec::new();
        let mut stack = vec![(root.clone(), 0usize)];
        let mut truncated = false;
        while let Some((directory, depth)) = stack.pop() {
            if is_cancelled(cancelled_walks, id) {
                return Err(FileError::Cancelled(format!(
                    "filesystem walk '{id}' was cancelled"
                )));
            }
            if started.elapsed() >= timeout {
                return Err(FileError::Timeout(format!(
                    "filesystem walk exceeded {} ms",
                    timeout.as_millis()
                )));
            }
            if depth >= max_depth {
                continue;
            }
            let children = match std::fs::read_dir(&directory) {
                Ok(children) => children,
                Err(error) if entries.is_empty() && directory == root => {
                    return Err(FileError::IoFailure(format!(
                        "read walk root {}: {error}",
                        directory.display()
                    )));
                }
                Err(_) => continue,
            };
            let mut paths = children
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            paths.sort_by_key(|path| path_key(path));
            for path in paths {
                if is_cancelled(cancelled_walks, id) {
                    return Err(FileError::Cancelled(format!(
                        "filesystem walk '{id}' was cancelled"
                    )));
                }
                if started.elapsed() >= timeout {
                    return Err(FileError::Timeout(format!(
                        "filesystem walk exceeded {} ms",
                        timeout.as_millis()
                    )));
                }
                if entries.len() >= max_results {
                    truncated = true;
                    break;
                }
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                let reparse = is_reparse_point(&metadata);
                if reparse {
                    entries.push(WalkEntry {
                        path: display_path(&path),
                        relative_path: path
                            .strip_prefix(&root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .into_owned(),
                        directory: false,
                        reparse_point: true,
                        size: metadata.len(),
                    });
                    continue;
                }
                let canonical = match std::fs::canonicalize(&path) {
                    Ok(canonical) if within(&canonical, &root) => canonical,
                    _ => continue,
                };
                let relative = canonical
                    .strip_prefix(&root)
                    .unwrap_or(&canonical)
                    .to_string_lossy()
                    .into_owned();
                let directory = metadata.is_dir() && !reparse;
                entries.push(WalkEntry {
                    path: display_path(&canonical),
                    relative_path: relative,
                    directory,
                    reparse_point: false,
                    size: metadata.len(),
                });
                if directory {
                    stack.push((canonical, depth + 1));
                }
            }
            if truncated {
                break;
            }
        }
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(WalkResult {
            root: PathBuf::from(display_path(&root)),
            entries,
            truncated,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WalkResult {
    root: PathBuf,
    entries: Vec<WalkEntry>,
    truncated: bool,
    elapsed_ms: u64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WalkEntry {
    path: String,
    relative_path: String,
    directory: bool,
    reparse_point: bool,
    size: u64,
}

fn validate_path_input(value: &str) -> FileResult<()> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES {
        return Err(FileError::InvalidInput(format!(
            "path must contain 1 to {MAX_PATH_BYTES} bytes"
        )));
    }
    if value.contains('\0') {
        return Err(FileError::InvalidInput(
            "path must not contain a NUL character".into(),
        ));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(FileError::InvalidInput("path must be absolute".into()));
    }
    let local_verbatim = value
        .strip_prefix(r"\\?\")
        .is_some_and(|path| !path.starts_with(r"UNC\") && !path.starts_with(r"\\"));
    if (value.starts_with(r"\\") || value.starts_with("//")) && !local_verbatim {
        return Err(FileError::InvalidInput(
            "UNC paths are not supported for plugin filesystem grants".into(),
        ));
    }
    Ok(())
}

fn canonical_existing(path: &Path, label: &str) -> FileResult<PathBuf> {
    let value = path.to_string_lossy();
    validate_path_input(&value)?;
    std::fs::canonicalize(path)
        .map_err(|error| FileError::NotFound(format!("{label} does not exist: {error}")))
}

fn within(path: &Path, root: &Path) -> bool {
    let path = path.components().collect::<Vec<_>>();
    let root = root.components().collect::<Vec<_>>();
    if path.len() < root.len() {
        return false;
    }
    path.iter().zip(root).all(|(path, root)| {
        if cfg!(windows) {
            path.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&root.as_os_str().to_string_lossy())
        } else {
            *path == root
        }
    })
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value.into_owned()
    }
}

pub(crate) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(value) = value.strip_prefix(r"\\?\") {
            if value.len() <= 240 {
                return value.to_owned();
            }
        }
    }
    value.into_owned()
}

fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn is_cancelled(cancelled_walks: &Arc<Mutex<BTreeSet<String>>>, id: &str) -> bool {
    cancelled_walks
        .lock()
        .map(|cancelled| cancelled.contains(id))
        .unwrap_or(false)
}

fn valid_walk_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    fn create_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    fn fixture() -> (PathBuf, FileAccess, Storage) {
        let root = std::env::temp_dir().join(format!(
            "steward-plugin-fs-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let data = root.join("data");
        let allowed = root.join("allowed");
        std::fs::create_dir_all(allowed.join("nested")).unwrap();
        std::fs::write(allowed.join("readme.txt"), "hello").unwrap();
        std::fs::write(root.join("secret.txt"), "secret").unwrap();
        let storage = Storage::open_at(&data).unwrap();
        let access = FileAccess {
            data_dir: data,
            plugin_id: "com.example.files".into(),
            publisher_fingerprint: "SHA256:publisher".into(),
            version: "1.0.0".into(),
        };
        (root, access, storage)
    }

    fn grant(storage: &Storage, access: &FileAccess, capability: &str, path: &Path) {
        storage
            .grant_capability(&NewCapabilityGrant {
                plugin_id: access.plugin_id.clone(),
                publisher_fingerprint: access.publisher_fingerprint.clone(),
                version_range: "^1".into(),
                capability: capability.into(),
                scope: format!("path={}", display_path(path)),
                granted_at: 1,
            })
            .unwrap();
    }

    #[test]
    fn reads_only_paths_under_active_roots_and_rechecks_revocation() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        let grant_row = storage
            .grant_capability(&NewCapabilityGrant {
                plugin_id: access.plugin_id.clone(),
                publisher_fingerprint: access.publisher_fingerprint.clone(),
                version_range: "^1".into(),
                capability: FS_READ.into(),
                scope: format!("path={}", display_path(&allowed)),
                granted_at: 1,
            })
            .unwrap();
        assert_eq!(
            access
                .read_text(allowed.join("readme.txt").to_str().unwrap(), 32)
                .unwrap(),
            "hello"
        );
        assert!(matches!(
            access.read_text(root.join("secret.txt").to_str().unwrap(), 32),
            Err(FileError::PermissionDenied(_))
        ));
        storage.revoke_capability(grant_row.id, 2).unwrap();
        assert!(matches!(
            access.read_text(allowed.join("readme.txt").to_str().unwrap(), 32),
            Err(FileError::PermissionDenied(_))
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn walk_is_bounded_and_never_descends_into_reparse_points() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        let cancelled = Arc::new(Mutex::new(BTreeSet::new()));
        let result = access
            .walk(
                &cancelled,
                "walk-1",
                allowed.to_str().unwrap(),
                100,
                32,
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(result
            .entries
            .iter()
            .any(|entry| entry.relative_path.ends_with("readme.txt")));
        assert!(!result.truncated);

        let limited = access
            .walk(
                &cancelled,
                "walk-2",
                allowed.to_str().unwrap(),
                1,
                32,
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(limited.entries.len(), 1);
        assert!(limited.truncated);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn write_requires_an_explicit_write_grant_and_rejects_traversal() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        assert!(matches!(
            access.write_text(allowed.join("new.txt").to_str().unwrap(), "new"),
            Err(FileError::PermissionDenied(_))
        ));
        grant(&storage, &access, FS_WRITE, &allowed);
        assert_eq!(
            access
                .write_text(allowed.join("new.txt").to_str().unwrap(), "new")
                .unwrap(),
            allowed.join("new.txt")
        );
        let traversal = allowed.join("..").join("escaped.txt");
        assert!(matches!(
            access.write_text(traversal.to_str().unwrap(), "escape"),
            Err(FileError::PermissionDenied(_))
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reparse_points_cannot_escape_or_extend_a_walk() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        let link = allowed.join("outside-link");
        if create_directory_link(&outside, &link).is_err() {
            std::fs::remove_dir_all(root).ok();
            return;
        }
        let result = access
            .walk(
                &Arc::new(Mutex::new(BTreeSet::new())),
                "walk-link",
                allowed.to_str().unwrap(),
                100,
                32,
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(!result
            .entries
            .iter()
            .any(|entry| entry.relative_path.ends_with("secret.txt")));
        assert!(result
            .entries
            .iter()
            .any(|entry| { entry.relative_path.ends_with("outside-link") && entry.reparse_point }));
        assert!(matches!(
            access.read_text(link.join("secret.txt").to_str().unwrap(), 32),
            Err(FileError::PermissionDenied(_))
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancellation_and_timeout_are_structured() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        let cancelled = Arc::new(Mutex::new(BTreeSet::from(["walk".into()])));
        assert!(matches!(
            access.walk(
                &cancelled,
                "walk",
                allowed.to_str().unwrap(),
                10,
                2,
                Duration::from_secs(1)
            ),
            Err(FileError::Cancelled(_))
        ));
        assert!(matches!(
            access.walk(
                &Arc::new(Mutex::new(BTreeSet::new())),
                "timeout",
                allowed.to_str().unwrap(),
                10,
                2,
                Duration::ZERO
            ),
            Err(FileError::Timeout(_))
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn invalid_path_shapes_fail_before_io() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        for path in [
            "relative.txt",
            r"\\server\share\file.txt",
            "D:\\bad\0path.txt",
        ] {
            assert!(matches!(
                access.read_text(path, 64),
                Err(FileError::InvalidInput(_))
            ));
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn case_variant_grants_collapse_to_one_canonical_root() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        let alias = PathBuf::from(display_path(&allowed).to_uppercase());
        grant(&storage, &access, FS_READ, &allowed);
        grant(&storage, &access, FS_READ, &alias);
        assert_eq!(access.matching_paths(FS_READ).unwrap().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn unicode_and_long_paths_stay_inside_the_grant() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        let mut parent = allowed.clone();
        for index in 0..20 {
            parent.push(format!("segment-{index:02}-资料"));
        }
        let long_parent = extended_path(&parent);
        std::fs::create_dir_all(&long_parent).unwrap();
        let file = extended_path(&parent.join("result.txt"));
        std::fs::write(&file, "long path").unwrap();
        assert_eq!(
            access.read_text(file.to_str().unwrap(), 64).unwrap(),
            "long path"
        );
        std::fs::remove_dir_all(root).ok();

        fn extended_path(path: &Path) -> PathBuf {
            let value = path.to_string_lossy();
            if value.starts_with(r"\\?\") {
                path.to_path_buf()
            } else {
                PathBuf::from(format!(r"\\?\{value}"))
            }
        }
    }

    #[test]
    fn two_unrelated_plugins_use_the_same_scoped_api() {
        let (root, access, storage) = fixture();
        let allowed = std::fs::canonicalize(root.join("allowed")).unwrap();
        grant(&storage, &access, FS_READ, &allowed);
        let second = FileAccess {
            plugin_id: "it.example.git-repos".into(),
            ..access.clone()
        };
        assert!(matches!(
            second.read_text(allowed.join("readme.txt").to_str().unwrap(), 32),
            Err(FileError::PermissionDenied(_))
        ));
        grant(&storage, &second, FS_READ, &allowed);
        assert_eq!(
            second
                .read_text(allowed.join("readme.txt").to_str().unwrap(), 32)
                .unwrap(),
            "hello"
        );
        assert_eq!(
            access
                .read_text(allowed.join("readme.txt").to_str().unwrap(), 32)
                .unwrap(),
            "hello"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
