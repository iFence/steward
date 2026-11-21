//! OS-level resource containment for one plugin-host process.

use std::time::{Duration, Instant};

const MIB: usize = 1024 * 1024;
const DEFAULT_MEMORY_LIMIT_BYTES: usize = 256 * MIB;
const DEFAULT_CPU_RATE_PERCENT: u32 = 50;
const DEFAULT_HANDLE_LIMIT: u32 = 1_024;
const HANDLE_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PluginResourcePolicy {
    pub(crate) memory_limit_bytes: usize,
    pub(crate) cpu_rate_percent: u32,
    pub(crate) active_process_limit: u32,
    pub(crate) handle_limit: u32,
    pub(crate) handle_poll_interval_ms: u64,
}

impl PluginResourcePolicy {
    pub(crate) fn from_env() -> Self {
        Self {
            memory_limit_bytes: env_number("STEWARD_PLUGIN_MEMORY_LIMIT_MIB")
                .map(|mib| (mib as usize).saturating_mul(MIB))
                .unwrap_or(DEFAULT_MEMORY_LIMIT_BYTES)
                .clamp(32 * MIB, 4 * 1024 * MIB),
            cpu_rate_percent: env_number("STEWARD_PLUGIN_CPU_RATE_PERCENT")
                .unwrap_or(DEFAULT_CPU_RATE_PERCENT as u64)
                .clamp(1, 100) as u32,
            active_process_limit: 1,
            handle_limit: env_number("STEWARD_PLUGIN_HANDLE_LIMIT")
                .unwrap_or(DEFAULT_HANDLE_LIMIT as u64)
                .clamp(64, 65_536) as u32,
            handle_poll_interval_ms: HANDLE_POLL_INTERVAL.as_millis() as u64,
        }
    }

    pub(crate) fn memory_limit_mib(self) -> usize {
        self.memory_limit_bytes / MIB
    }
}

fn env_number(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// Owns the Windows Job Object assigned to one plugin-host child.
pub(crate) struct PluginProcessContainment {
    pid: u32,
    policy: PluginResourcePolicy,
    next_handle_check: Instant,
    violation: Option<String>,
    #[cfg(target_os = "windows")]
    job: windows_sys::Win32::Foundation::HANDLE,
}

impl PluginProcessContainment {
    pub(crate) fn attach(pid: u32) -> anyhow::Result<Self> {
        let policy = PluginResourcePolicy::from_env();
        let mut containment = Self {
            pid,
            policy,
            next_handle_check: Instant::now(),
            violation: None,
            #[cfg(target_os = "windows")]
            job: std::ptr::null_mut(),
        };
        containment.configure()?;
        Ok(containment)
    }

    pub(crate) fn policy(&self) -> PluginResourcePolicy {
        self.policy
    }

    pub(crate) fn backend(&self) -> &'static str {
        if cfg!(target_os = "windows") {
            "windows_job_object"
        } else {
            "unsupported"
        }
    }

    /// Poll limits that Windows Job Objects cannot report through process exit
    /// alone. A violation terminates the whole job before returning.
    pub(crate) fn poll_limit_violation(&mut self) -> Option<String> {
        if let Some(violation) = &self.violation {
            return Some(violation.clone());
        }
        let now = Instant::now();
        if now < self.next_handle_check {
            return None;
        }
        self.next_handle_check = now + HANDLE_POLL_INTERVAL;

        #[cfg(target_os = "windows")]
        if let Some(handle_count) = process_handle_count(self.pid) {
            if handle_count > self.policy.handle_limit {
                self.violation = Some(format!(
                    "plugin host exceeded its {} handle limit ({handle_count} handles)",
                    self.policy.handle_limit
                ));
                terminate_job(self.job);
                return self.violation.clone();
            }
        }
        None
    }

    /// Return `(resource_limit, message)` for an exited child.
    pub(crate) fn describe_exit(
        &mut self,
        status: Option<std::process::ExitStatus>,
        fallback: &str,
    ) -> (bool, String) {
        #[cfg(target_os = "windows")]
        if let Some(status) = status {
            self.refresh_job_violation();
            let raw_exit_code = status.code().map(|code| code as u32);
            if self.policy.memory_limit_bytes < DEFAULT_MEMORY_LIMIT_BYTES
                && raw_exit_code.is_some_and(|code| {
                    matches!(
                        code,
                        0xC000_0017 // STATUS_NO_MEMORY
                            | 0xC000_012D // STATUS_COMMITMENT_LIMIT
                            | 0xC000_0409 // STATUS_STACK_BUFFER_OVERRUN / fail-fast
                    )
                })
            {
                self.violation = Some(format!(
                    "plugin host exhausted its {} MiB memory limit",
                    self.policy.memory_limit_mib()
                ));
            }
        }
        if let Some(violation) = &self.violation {
            return (true, violation.clone());
        }
        let status = status
            .map(|status| status.to_string())
            .unwrap_or_else(|| fallback.into());
        (
            false,
            format!("plugin host process exited unexpectedly with {status}"),
        )
    }

    #[cfg(target_os = "windows")]
    fn configure(&mut self) -> anyhow::Result<()> {
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectCpuRateControlInformation,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
            JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = self.policy.active_process_limit;
        limits.ProcessMemoryLimit = self.policy.memory_limit_bytes;
        limits.JobMemoryLimit = self.policy.memory_limit_bytes;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe {
                CloseHandle(job);
            }
            return Err(std::io::Error::last_os_error().into());
        }

        let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                CpuRate: self.policy.cpu_rate_percent * 100,
            },
        };
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectCpuRateControlInformation,
                (&cpu as *const JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe {
                CloseHandle(job);
            }
            return Err(std::io::Error::last_os_error().into());
        }

        let process: HANDLE = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA
                    | PROCESS_TERMINATE
                    | PROCESS_QUERY_INFORMATION
                    | PROCESS_QUERY_LIMITED_INFORMATION,
                FALSE,
                self.pid,
            )
        };
        if process.is_null() {
            unsafe {
                CloseHandle(job);
            }
            return Err(std::io::Error::last_os_error().into());
        }
        let assigned = unsafe { AssignProcessToJobObject(job, process) };
        unsafe {
            CloseHandle(process);
        }
        if assigned == 0 {
            unsafe {
                CloseHandle(job);
            }
            return Err(std::io::Error::last_os_error().into());
        }

        self.job = job;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn refresh_job_violation(&mut self) {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectExtendedLimitInformation, JobObjectLimitViolationInformation,
            QueryInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOBOBJECT_LIMIT_VIOLATION_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };

        let mut info = JOBOBJECT_LIMIT_VIOLATION_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.job,
                JobObjectLimitViolationInformation,
                (&mut info as *mut JOBOBJECT_LIMIT_VIOLATION_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_LIMIT_VIOLATION_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if queried != 0
            && info.ViolationLimitFlags
                & (JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY)
                != 0
        {
            self.violation = Some(format!(
                "plugin host exceeded its {} MiB memory limit",
                self.policy.memory_limit_mib()
            ));
            return;
        }

        // Windows does not always populate ViolationLimitFlags when a
        // memory-limited process aborts before returning an allocation error to
        // its own runtime. The job's peak accounting is still authoritative.
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if queried != 0
            && limits.PeakProcessMemoryUsed.max(limits.PeakJobMemoryUsed)
                >= self.policy.memory_limit_bytes
        {
            self.violation = Some(format!(
                "plugin host reached its {} MiB memory limit",
                self.policy.memory_limit_mib()
            ));
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn configure(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Drop for PluginProcessContainment {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if !self.job.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.job);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn process_handle_count(pid: u32) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{
        GetProcessHandleCount, OpenProcess, PROCESS_QUERY_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            FALSE,
            pid,
        )
    };
    if process.is_null() {
        return None;
    }
    let mut count = 0;
    let queried = unsafe { GetProcessHandleCount(process, &mut count) };
    unsafe {
        CloseHandle(process);
    }
    (queried != 0).then_some(count)
}

#[cfg(target_os = "windows")]
fn terminate_job(job: windows_sys::Win32::Foundation::HANDLE) {
    if job.is_null() {
        return;
    }
    unsafe {
        windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn job_object_enforces_limits_and_owns_the_child_lifetime() {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
            JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn containment test child");
        let containment = match PluginProcessContainment::attach(child.id()) {
            Ok(containment) => containment,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("attach Job Object: {error:#}");
            }
        };

        assert_eq!(containment.backend(), "windows_job_object");
        assert!(process_handle_count(child.id()).is_some());
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                containment.job,
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(queried, 0);
        let expected_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        assert_eq!(
            limits.BasicLimitInformation.LimitFlags & expected_flags,
            expected_flags
        );
        assert_eq!(limits.BasicLimitInformation.ActiveProcessLimit, 1);
        assert_eq!(
            limits.ProcessMemoryLimit,
            containment.policy.memory_limit_bytes
        );
        assert_eq!(limits.JobMemoryLimit, containment.policy.memory_limit_bytes);

        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                containment.job,
                JobObjectCpuRateControlInformation,
                (&mut cpu as *mut JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(queried, 0);
        assert_eq!(
            cpu.ControlFlags,
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
        );
        assert_eq!(
            unsafe { cpu.Anonymous.CpuRate },
            containment.policy.cpu_rate_percent * 100
        );

        drop(containment);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait().expect("inspect containment child") {
                Some(_) => break,
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("closing the Job Object did not terminate its child");
                }
            }
        }
    }
}
