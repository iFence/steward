//! Launcher-owned execution broker for isolated plugin host processes.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use steward_plugin_contract::capabilities::{EXEC_CANCEL, EXEC_RUN};
use steward_plugin_contract::{
    PluginErrorCode, PluginExecutionRequest, PluginExecutionResult, PluginSessionId,
};
use steward_plugin_host::PluginAuthorization;

use crate::plugin_filesystem::FileAccess;

const EXECUTION_WORKERS: usize = 2;
const EXECUTION_QUEUE_DEPTH: usize = 8;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_OUTPUT_BYTES: u64 = 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 32 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) type ExecutionResultSink = Arc<dyn Fn(u64, PluginExecutionResult) + Send + Sync>;
pub(crate) type ExecutionAudit = Arc<dyn Fn(&str, &str, &str, &str) + Send + Sync>;

struct ExecutionControl {
    cancelled: AtomicBool,
}

struct ExecutionTask {
    request: PluginExecutionRequest,
    control: Arc<ExecutionControl>,
}

pub(crate) struct ExecutionBroker {
    tasks: crossbeam_channel::Sender<ExecutionTask>,
    controls: Arc<Mutex<HashMap<u64, Arc<ExecutionControl>>>>,
    results: ExecutionResultSink,
}

impl ExecutionBroker {
    pub(crate) fn new(
        data_dir: PathBuf,
        authorization: PluginAuthorization,
        session_id: PluginSessionId,
        results: ExecutionResultSink,
        audit: ExecutionAudit,
    ) -> Self {
        let access = FileAccess {
            data_dir: data_dir.clone(),
            plugin_id: authorization.plugin_id.clone(),
            publisher_fingerprint: authorization.publisher_fingerprint.clone(),
            version: authorization.version.clone(),
        };
        let (tasks, receiver) = crossbeam_channel::bounded::<ExecutionTask>(EXECUTION_QUEUE_DEPTH);
        let controls = Arc::new(Mutex::new(HashMap::<u64, Arc<ExecutionControl>>::new()));
        let broker_results = results.clone();
        for worker in 0..EXECUTION_WORKERS {
            let receiver = receiver.clone();
            let controls = controls.clone();
            let data_dir = data_dir.clone();
            let authorization = authorization.clone();
            let access = access.clone();
            let results = results.clone();
            let audit = audit.clone();
            std::thread::Builder::new()
                .name(format!("steward-plugin-exec-{worker}"))
                .spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        let result = execute(
                            &data_dir,
                            &authorization,
                            &access,
                            session_id,
                            &task.request,
                            &task.control,
                            &audit,
                        );
                        if let Ok(mut controls) = controls.lock() {
                            controls.remove(&task.request.request_id);
                        }
                        results(task.request.request_id, result);
                    }
                })
                .expect("spawn plugin execution worker");
        }
        Self {
            tasks,
            controls,
            results: broker_results,
        }
    }

    pub(crate) fn submit(&self, request: PluginExecutionRequest) {
        let request_id = request.request_id;
        let control = Arc::new(ExecutionControl {
            cancelled: AtomicBool::new(false),
        });
        {
            let Ok(mut controls) = self.controls.lock() else {
                (self.results)(
                    request_id,
                    PluginExecutionResult::failure(
                        PluginErrorCode::HostFailure,
                        "execution broker state is unavailable",
                    ),
                );
                return;
            };
            if controls.contains_key(&request_id) {
                (self.results)(
                    request_id,
                    PluginExecutionResult::failure(
                        PluginErrorCode::InvalidInput,
                        "process request id is already active",
                    ),
                );
                return;
            }
            controls.insert(request_id, control.clone());
        }
        let task = ExecutionTask { request, control };
        if let Err(crossbeam_channel::TrySendError::Full(task))
        | Err(crossbeam_channel::TrySendError::Disconnected(task)) = self.tasks.try_send(task)
        {
            if let Ok(mut controls) = self.controls.lock() {
                controls.remove(&task.request.request_id);
            }
            (self.results)(
                task.request.request_id,
                PluginExecutionResult::failure(
                    PluginErrorCode::ResourceExhausted,
                    "the controlled process queue is full; retry later",
                ),
            );
        }
    }

    pub(crate) fn cancel(&self, request_id: u64) {
        if let Ok(controls) = self.controls.lock() {
            if let Some(control) = controls.get(&request_id) {
                control.cancelled.store(true, Ordering::Release);
            }
        }
    }

    pub(crate) fn cancel_all(&self) {
        if let Ok(controls) = self.controls.lock() {
            for control in controls.values() {
                control.cancelled.store(true, Ordering::Release);
            }
        }
    }
}

impl Drop for ExecutionBroker {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

fn execute(
    data_dir: &Path,
    authorization: &PluginAuthorization,
    access: &FileAccess,
    _session_id: PluginSessionId,
    request: &PluginExecutionRequest,
    control: &ExecutionControl,
    audit: &ExecutionAudit,
) -> PluginExecutionResult {
    let started = Instant::now();
    let scope = format!(
        "executable={};cwd={}",
        request.executable, request.working_directory
    );
    let validated = validate_request(data_dir, authorization, access, request).and_then(
        |(executable, working_directory)| spawn(request, control, &executable, &working_directory),
    );
    let mut result = match validated {
        Ok(result) => result,
        Err(error) => PluginExecutionResult::failure(error.code, error.message),
    };
    result.duration_ms = started.elapsed().as_millis() as u64;
    let outcome = if result.error_code == Some(PluginErrorCode::PermissionDenied) {
        "denied"
    } else if result.error_code.is_some() {
        "failed"
    } else {
        "allowed"
    };
    audit(
        EXEC_RUN,
        &scope,
        outcome,
        result.error.as_deref().unwrap_or("origin=broker"),
    );
    if result.cancelled {
        audit(
            EXEC_CANCEL,
            &format!("request={}", request.request_id),
            "allowed",
            "origin=broker",
        );
    }
    result
}

#[derive(Debug)]
struct ExecutionError {
    code: PluginErrorCode,
    message: String,
}

impl ExecutionError {
    fn denied(message: impl Into<String>) -> Self {
        Self {
            code: PluginErrorCode::PermissionDenied,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: PluginErrorCode::InvalidInput,
            message: message.into(),
        }
    }

    fn resource(message: impl Into<String>) -> Self {
        Self {
            code: PluginErrorCode::ResourceExhausted,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: PluginErrorCode::HostFailure,
            message: message.into(),
        }
    }
}

fn validate_request(
    data_dir: &Path,
    authorization: &PluginAuthorization,
    access: &FileAccess,
    request: &PluginExecutionRequest,
) -> Result<(PathBuf, PathBuf), ExecutionError> {
    if request.request_id == 0 {
        return Err(ExecutionError::invalid(
            "process request id must be non-zero",
        ));
    }
    if request.arguments.len() > MAX_ARGUMENTS {
        return Err(ExecutionError::resource("process has too many arguments"));
    }
    if request.arguments.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES {
        return Err(ExecutionError::resource(
            "process arguments exceed the 32 KiB limit",
        ));
    }
    if request
        .arguments
        .iter()
        .any(|argument| argument.chars().any(|character| character == '\0'))
    {
        return Err(ExecutionError::invalid(
            "process arguments cannot contain NUL",
        ));
    }
    if request.timeout_ms == 0 || request.timeout_ms > MAX_TIMEOUT_MS {
        return Err(ExecutionError::invalid(format!(
            "process timeoutMs must be between 1 and {MAX_TIMEOUT_MS}"
        )));
    }
    if request.max_output_bytes == 0 || request.max_output_bytes > MAX_OUTPUT_BYTES {
        return Err(ExecutionError::invalid(format!(
            "process maxOutputBytes must be between 1 and {MAX_OUTPUT_BYTES}"
        )));
    }

    let active = authorization
        .active_grants(data_dir)
        .map_err(|error| ExecutionError::failed(error.to_string()))?;
    if !active.contains(&(EXEC_RUN.into(), "host=steward".into())) {
        return Err(ExecutionError::denied(
            "no active steward.exec.run host permission",
        ));
    }
    let executable = request.executable.as_str();
    if !authorization
        .declared_capabilities
        .process
        .execute
        .iter()
        .any(|declared| declared == executable)
        || !active.contains(&(EXEC_RUN.into(), format!("executable={executable}")))
    {
        return Err(ExecutionError::denied(format!(
            "executable '{executable}' does not have an active exact grant"
        )));
    }
    let executable = resolve_executable(executable)?;
    let working_directory = access
        .authorize_write_directory(&request.working_directory)
        .map_err(|error| match error {
            crate::plugin_filesystem::FileError::PermissionDenied(message)
            | crate::plugin_filesystem::FileError::InvalidInput(message) => {
                ExecutionError::denied(message)
            }
            error => ExecutionError::failed(error.host_message()),
        })?;
    Ok((executable, working_directory))
}

fn resolve_executable(requested: &str) -> Result<PathBuf, ExecutionError> {
    let requested = requested.trim();
    if requested.is_empty() || requested.len() > 260 {
        return Err(ExecutionError::invalid(
            "process executable name is empty or too long",
        ));
    }
    if !requested.to_ascii_lowercase().ends_with(".exe") || is_shell_executable(requested) {
        return Err(ExecutionError::denied(
            "controlled execution only accepts non-shell .exe targets",
        ));
    }
    let path = Path::new(requested);
    if path.is_absolute() {
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            ExecutionError::failed(format!("resolve executable '{}': {error}", path.display()))
        })?;
        return canonical
            .is_file()
            .then_some(canonical)
            .ok_or_else(|| ExecutionError::failed("approved executable is not a file"));
    }
    if path.components().count() != 1 {
        return Err(ExecutionError::invalid(
            "relative process executables must be exact file names",
        ));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return Err(ExecutionError::failed("PATH is unavailable"));
    };
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(requested);
        if !candidate
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(requested))
        {
            continue;
        }
        if let Ok(candidate) = std::fs::canonicalize(candidate) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(ExecutionError::failed(format!(
        "executable '{requested}' is not on PATH"
    )))
}

fn is_shell_executable(executable: &str) -> bool {
    let name = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "wsl.exe"
            | "bash.exe"
            | "sh.exe"
            | "cscript.exe"
            | "wscript.exe"
            | "mshta.exe"
            | "rundll32.exe"
            | "python.exe"
            | "python3.exe"
            | "py.exe"
            | "node.exe"
            | "nodejs.exe"
            | "deno.exe"
            | "bun.exe"
            | "perl.exe"
            | "ruby.exe"
            | "java.exe"
            | "javaw.exe"
            | "dotnet.exe"
            | "powershell_ise.exe"
            | "wt.exe"
            | "conhost.exe"
    )
}

fn spawn(
    request: &PluginExecutionRequest,
    control: &ExecutionControl,
    executable: &Path,
    working_directory: &Path,
) -> Result<PluginExecutionResult, ExecutionError> {
    let timeout_ms = if request.timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        request.timeout_ms
    };
    let max_output_bytes = if request.max_output_bytes == 0 {
        DEFAULT_OUTPUT_BYTES
    } else {
        request.max_output_bytes
    };
    let started = Instant::now();
    let mut command = Command::new(executable);
    command
        .args(&request.arguments)
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for name in [
        "SystemRoot",
        "WINDIR",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "SystemDrive",
        "COMSPEC",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| ExecutionError::failed(format!("spawn approved executable: {error}")))?;
    let mut job = Some(ProcessJob::attach(&child).map_err(|error| {
        let _ = child.kill();
        ExecutionError::failed(format!("contain approved executable: {error}"))
    })?);

    let output_bytes = Arc::new(AtomicU64::new(0));
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecutionError::failed("process stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ExecutionError::failed("process stderr is unavailable"))?;
    let stdout_reader = spawn_output_reader(
        stdout,
        max_output_bytes,
        output_bytes.clone(),
        output_exceeded.clone(),
    );
    let stderr_reader = spawn_output_reader(
        stderr,
        max_output_bytes,
        output_bytes,
        output_exceeded.clone(),
    );

    let deadline = started + Duration::from_millis(timeout_ms);
    let (exit_code, timed_out, cancelled) = loop {
        if control.cancelled.load(Ordering::Acquire) {
            terminate_process_tree(&mut child, job.as_ref());
            break (None, false, true);
        }
        if output_exceeded.load(Ordering::Acquire) {
            terminate_process_tree(&mut child, job.as_ref());
            break (None, false, false);
        }
        if Instant::now() >= deadline {
            terminate_process_tree(&mut child, job.as_ref());
            break (None, true, false);
        }
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), false, false),
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                terminate_process_tree(&mut child, job.as_ref());
                return Err(ExecutionError::failed(format!(
                    "inspect controlled process: {error}"
                )));
            }
        }
    };
    if timed_out || cancelled || output_exceeded.load(Ordering::Acquire) {
        let _ = child.wait();
    }
    drop(job.take());
    let stdout = join_output_reader(stdout_reader);
    let stderr = join_output_reader(stderr_reader);

    if output_exceeded.load(Ordering::Acquire) {
        return Err(ExecutionError::resource(format!(
            "process output exceeded the {max_output_bytes} byte limit"
        )));
    }
    let mut result = PluginExecutionResult {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
        cancelled,
        error_code: None,
        error: None,
    };
    if timed_out {
        result.error_code = Some(PluginErrorCode::Timeout);
        result.error = Some(format!("process exceeded its {timeout_ms} ms timeout"));
    } else if cancelled {
        result.error_code = Some(PluginErrorCode::Cancelled);
        result.error = Some("process was cancelled".into());
    }
    Ok(result)
}

fn spawn_output_reader(
    mut reader: impl Read + Send + 'static,
    max_output_bytes: u64,
    total: Arc<AtomicU64>,
    exceeded: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let total = total.fetch_add(read as u64, Ordering::AcqRel) + read as u64;
                    if total > max_output_bytes {
                        exceeded.store(true, Ordering::Release);
                    } else {
                        output.extend_from_slice(&buffer[..read]);
                    }
                }
            }
        }
        output
    })
}

fn join_output_reader(reader: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    reader.join().unwrap_or_default()
}

fn terminate_process_tree(child: &mut Child, job: Option<&ProcessJob>) {
    if let Some(job) = job {
        job.terminate();
    }
    let _ = child.kill();
}

#[cfg(target_os = "windows")]
struct ProcessJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(not(target_os = "windows"))]
struct ProcessJob;

#[cfg(not(target_os = "windows"))]
impl ProcessJob {
    fn attach(_child: &Child) -> std::io::Result<Self> {
        Ok(Self)
    }

    fn terminate(&self) {}
}

#[cfg(target_os = "windows")]
impl ProcessJob {
    fn attach(child: &Child) -> std::io::Result<Self> {
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe {
                CloseHandle(handle);
            }
            return Err(std::io::Error::last_os_error());
        }
        let process: HANDLE = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                FALSE,
                child.id(),
            )
        };
        if process.is_null() {
            unsafe {
                CloseHandle(handle);
            }
            return Err(std::io::Error::last_os_error());
        }
        let assigned = unsafe { AssignProcessToJobObject(handle, process) };
        unsafe {
            CloseHandle(process);
        }
        if assigned == 0 {
            unsafe {
                CloseHandle(handle);
            }
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_and_script_targets_are_rejected() {
        for executable in [
            "cmd.exe",
            "powershell.exe",
            "C:\\Windows\\System32\\cmd.exe",
            "git",
            "git.bat",
            "..\\git.exe",
        ] {
            assert!(
                resolve_executable(executable).is_err(),
                "unexpected executable acceptance: {executable}"
            );
        }
    }

    #[test]
    fn request_limits_are_checked_before_authorization_or_spawning() {
        let data_dir = std::env::temp_dir().join("steward-execution-limits");
        let authorization = PluginAuthorization {
            plugin_id: "com.example.test".into(),
            publisher_fingerprint: "SHA256:publisher".into(),
            version: "1.0.0".into(),
            features: Vec::new(),
            legacy_compatibility: false,
            declared_capabilities: Default::default(),
            granted_capabilities: Default::default(),
        };
        let access = FileAccess {
            data_dir,
            plugin_id: authorization.plugin_id.clone(),
            publisher_fingerprint: authorization.publisher_fingerprint.clone(),
            version: authorization.version.clone(),
        };
        let request = PluginExecutionRequest {
            request_id: 1,
            executable: "git.exe".into(),
            arguments: Vec::new(),
            working_directory: "C:\\".into(),
            timeout_ms: MAX_TIMEOUT_MS + 1,
            max_output_bytes: DEFAULT_OUTPUT_BYTES,
        };
        assert!(matches!(
            validate_request(Path::new("."), &authorization, &access, &request),
            Err(ExecutionError {
                code: PluginErrorCode::InvalidInput,
                ..
            })
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn execution_job_terminates_the_whole_process_tree() {
        use std::os::windows::process::CommandExt;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE, WAIT_OBJECT_0};
        use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

        let pid_path = std::env::temp_dir().join(format!(
            "steward-exec-tree-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stderr_path = pid_path.with_extension("stderr.txt");
        let child_pid_path = pid_path.to_string_lossy().replace('\'', "''");
        let stderr = std::fs::File::create(&stderr_path).unwrap();
        let mut root = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Set-Content -LiteralPath '{child_pid_path}' -Value 'started'; \
                     Start-Sleep -Milliseconds 500; \
                     $p=Start-Process ping.exe -ArgumentList '-n','20','127.0.0.1' \
                     -WindowStyle Hidden -PassThru; \
                     Set-Content -LiteralPath '{child_pid_path}' -Value $p.Id; \
                     Start-Sleep -Seconds 20"
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let job = ProcessJob::attach(&root).unwrap();

        let deadline = Instant::now() + Duration::from_secs(20);
        let descendant_pid = loop {
            if let Ok(value) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = value.trim().parse::<u32>() {
                    break pid;
                }
            }
            if Instant::now() >= deadline {
                let error = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                let marker = std::fs::read_to_string(&pid_path).unwrap_or_default();
                let status = root.try_wait().ok().flatten();
                panic!(
                    "descendant process was not created; status={status:?}; marker={marker}; stderr={error}"
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        job.terminate();
        let _ = root.kill();
        let _ = root.wait();

        let descendant = unsafe { OpenProcess(SYNCHRONIZE, FALSE, descendant_pid) };
        if !descendant.is_null() {
            let waited = unsafe { WaitForSingleObject(descendant, 5_000) };
            unsafe {
                CloseHandle(descendant);
            }
            assert_eq!(waited, WAIT_OBJECT_0, "descendant survived job termination");
        }
        std::fs::remove_file(pid_path).ok();
        std::fs::remove_file(stderr_path).ok();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn broker_runs_git_state_and_clone_without_a_shell() {
        let Ok(git) = resolve_executable("git.exe") else {
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "steward-exec-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        let clone = root.join("clone");
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::create_dir_all(&root).unwrap();

        run_git(&git, &repository, &["init"]);
        run_git(
            &git,
            &repository,
            &[
                "-c",
                "user.name=Steward Test",
                "-c",
                "user.email=steward@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        run_git(&git, &root, &["init", "--bare", "remote.git"]);
        let remote_arg = remote.to_string_lossy().into_owned();
        run_git(&git, &repository, &["remote", "add", "origin", &remote_arg]);
        run_git(&git, &repository, &["push", "-u", "origin", "HEAD"]);

        let status = run_git(&git, &repository, &["status", "--porcelain=v1", "--branch"]);
        assert!(status.contains("##"));
        let remotes = run_git(&git, &repository, &["remote", "-v"]);
        assert!(remotes.contains("origin"));
        let unpushed = run_git(&git, &repository, &["rev-list", "--count", "@{u}..HEAD"]);
        assert_eq!(unpushed.trim(), "0");

        let source_arg = repository.to_string_lossy().into_owned();
        let clone_arg = clone.to_string_lossy().into_owned();
        run_git(&git, &root, &["clone", "--", &source_arg, &clone_arg]);
        assert!(clone.join(".git").is_dir());
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(target_os = "windows")]
    fn run_git(executable: &Path, working_directory: &Path, arguments: &[&str]) -> String {
        let request = PluginExecutionRequest {
            request_id: 1,
            executable: executable.to_string_lossy().into_owned(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            working_directory: working_directory.to_string_lossy().into_owned(),
            timeout_ms: 30_000,
            max_output_bytes: 1024 * 1024,
        };
        let control = ExecutionControl {
            cancelled: AtomicBool::new(false),
        };
        let result = spawn(&request, &control, executable, working_directory).unwrap();
        assert_eq!(
            result.exit_code,
            Some(0),
            "git {:?} failed: {}",
            arguments,
            result.stderr
        );
        result.stdout
    }
}
