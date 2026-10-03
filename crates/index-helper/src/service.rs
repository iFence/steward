//! Windows service entry point.
//!
//! The MSI installs `steward-index-helper.exe --service` as an auto-start
//! LocalSystem service, which is what makes the `$MFT` fast path available on a
//! normal (non-elevated) launch. The process serves the same named pipe as the
//! standalone mode; only the privilege and the lifetime differ.

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};

/// Service name registered with the SCM (also used by the MSI and `sc.exe`).
pub const SERVICE_NAME: &str = "StewardIndexHelper";

/// Set by the control handler; the accept loop checks it between connections.
static STOP: AtomicBool = AtomicBool::new(false);

/// Connect to the SCM and run until stopped. Fails when the process was not
/// started by the SCM (e.g. run from a console), which is expected.
pub fn run() -> io::Result<()> {
    let name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_ptr() as *mut u16,
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    let started = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if started == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let name = wide(SERVICE_NAME);
    let handle =
        RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null());
    if handle.is_null() {
        return;
    }
    set_status(handle, SERVICE_START_PENDING, 0);
    set_status(handle, SERVICE_RUNNING, 0);
    while !STOP.load(Ordering::SeqCst) {
        let _ = crate::server::serve_once(crate::protocol::PIPE_NAME);
    }
    set_status(handle, SERVICE_STOPPED, 0);
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        STOP.store(true, Ordering::SeqCst);
        // `serve_once` blocks in `ConnectNamedPipe`; a throwaway client wakes it
        // so the loop can observe the flag and return promptly.
        std::thread::spawn(|| {
            for _ in 0..40 {
                if std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(crate::protocol::PIPE_NAME)
                    .is_ok()
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
    0
}

fn set_status(handle: SERVICE_STATUS_HANDLE, state: u32, checkpoint: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP
        } else {
            0
        },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: checkpoint,
        dwWaitHint: 0,
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
