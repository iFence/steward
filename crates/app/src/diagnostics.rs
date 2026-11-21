//! Privacy-bounded support diagnostics for Windows installation and runtime
//! failures.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::autostart;

const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;
const MAX_HASHED_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_RECENT_ERRORS: usize = 80;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticReport {
    schema_version: u32,
    generated_at_unix_ms: u128,
    application: ApplicationDiagnostic,
    operating_system: OsDiagnostic,
    data: DataDiagnostic,
    autostart: AutostartDiagnostic,
    trust: TrustDiagnostic,
    displays: DisplayDiagnostic,
    plugin_cache: PluginCacheDiagnostic,
    plugins: Vec<PluginDiagnostic>,
    log_files: Vec<LogFileDiagnostic>,
    recent_errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationDiagnostic {
    version: String,
    executable: PathBuf,
    executable_sha256: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct OsDiagnostic {
    family: String,
    architecture: String,
    version: Option<String>,
    build: Option<u32>,
    process_dpi_awareness: Option<String>,
    keyboard_layout: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DataDiagnostic {
    data_dir: PathBuf,
    database: PathBuf,
    database_exists: bool,
    database_schema_version: Option<i64>,
    database_error: Option<String>,
    plugins_dir: PathBuf,
    logs_dir: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutostartDiagnostic {
    enabled: bool,
    stored_value: Option<String>,
    resolved_path: Option<PathBuf>,
    points_to_existing_file: bool,
    points_to_current_executable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrustDiagnostic {
    embedded_root_count: usize,
    embedded_roots_sha256: String,
    override_configured: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayDiagnostic {
    monitor_count: usize,
    mixed_dpi: bool,
    monitors: Vec<MonitorDiagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorDiagnostic {
    index: u32,
    primary: bool,
    bounds: PixelRect,
    work_area: PixelRect,
    dpi: u32,
    scale_percent: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct PixelRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginDiagnostic {
    directory: String,
    id: Option<String>,
    version: Option<String>,
    package: PathBuf,
    package_exists: bool,
    package_size: Option<u64>,
    package_sha256: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginCacheDiagnostic {
    database: PathBuf,
    database_exists: bool,
    route_count: usize,
    routes: Vec<PluginRouteDiagnostic>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginRouteDiagnostic {
    id: String,
    version: String,
    package: PathBuf,
    package_exists: bool,
    commands: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogFileDiagnostic {
    name: String,
    size: u64,
}

/// Write a support report and return the created JSON path.
pub(crate) fn write_report(data_dir: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_report_path(data_dir));
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).context("create diagnostics output directory")?;
    }

    let report = collect(data_dir);
    let bytes = serde_json::to_vec_pretty(&report).context("serialize diagnostic report")?;
    std::fs::write(&output, bytes)
        .with_context(|| format!("write diagnostic report {}", output.display()))?;
    Ok(output)
}

fn default_report_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("diagnostics")
        .join(format!("steward-diagnostics-{}.json", unix_millis()))
}

fn collect(data_dir: &Path) -> DiagnosticReport {
    let executable = std::env::current_exe().unwrap_or_default();
    let plugins_dir = std::env::var_os("STEWARD_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("plugins"));
    let logs_dir = data_dir.join("logs");
    let database = steward_storage::Storage::database_path(data_dir);
    let (database_schema_version, database_error) =
        match steward_storage::Storage::existing_schema_version(data_dir) {
            Ok(version) => (version, None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
    let embedded_roots = include_str!("../../../assets/nsh/trust-roots.json");
    let embedded_root_count = serde_json::from_str::<Vec<String>>(embedded_roots)
        .map(|roots| roots.len())
        .unwrap_or(0);
    let logs = log_diagnostics(&logs_dir);
    let autostart = autostart::diagnostic();
    let displays = collect_displays();
    let plugin_cache = plugin_cache_diagnostics(data_dir);

    DiagnosticReport {
        schema_version: DIAGNOSTIC_SCHEMA_VERSION,
        generated_at_unix_ms: unix_millis(),
        application: ApplicationDiagnostic {
            version: env!("CARGO_PKG_VERSION").into(),
            executable: executable.clone(),
            executable_sha256: sha256_file(&executable).ok(),
        },
        operating_system: collect_os(),
        data: DataDiagnostic {
            data_dir: data_dir.to_path_buf(),
            database: database.clone(),
            database_exists: database.is_file(),
            database_schema_version,
            database_error,
            plugins_dir: plugins_dir.clone(),
            logs_dir: logs_dir.clone(),
        },
        autostart: AutostartDiagnostic {
            enabled: autostart.enabled,
            stored_value: autostart.stored_value,
            resolved_path: autostart.resolved_path,
            points_to_existing_file: autostart.points_to_existing_file,
            points_to_current_executable: autostart.points_to_current_executable,
        },
        trust: TrustDiagnostic {
            embedded_root_count,
            embedded_roots_sha256: sha256_bytes(embedded_roots.as_bytes()),
            override_configured: std::env::var_os("STEWARD_NSH_TRUST_ROOT").is_some(),
        },
        displays,
        plugin_cache,
        plugins: plugin_diagnostics(&plugins_dir),
        log_files: logs.0,
        recent_errors: logs.1,
    }
}

fn plugin_cache_diagnostics(data_dir: &Path) -> PluginCacheDiagnostic {
    let database = steward_plugin_registry::Registry::cache_path(data_dir);
    let database_exists = database.is_file();
    match steward_plugin_registry::Registry::cached_plugins_at(data_dir) {
        Ok(plugins) => PluginCacheDiagnostic {
            database,
            database_exists,
            route_count: plugins
                .iter()
                .map(|plugin| plugin.manifest.commands.len())
                .sum(),
            routes: plugins
                .into_iter()
                .map(|plugin| PluginRouteDiagnostic {
                    id: plugin.manifest.id,
                    version: plugin.manifest.version,
                    package_exists: plugin.package.is_file(),
                    package: plugin.package,
                    commands: plugin
                        .manifest
                        .commands
                        .into_iter()
                        .map(|command| command.name)
                        .collect(),
                })
                .collect(),
            error: None,
        },
        Err(error) => PluginCacheDiagnostic {
            database_exists,
            database,
            route_count: 0,
            routes: Vec::new(),
            error: Some(format!("{error:#}")),
        },
    }
}

fn log_diagnostics(logs_dir: &Path) -> (Vec<LogFileDiagnostic>, Vec<String>) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();

    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        files.push(LogFileDiagnostic {
            name,
            size: metadata.len(),
        });
        errors.extend(read_error_tail(&path));
    }
    if errors.len() > MAX_RECENT_ERRORS {
        errors.drain(..errors.len() - MAX_RECENT_ERRORS);
    }
    (files, errors)
}

fn read_error_tail(path: &Path) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if size > LOG_TAIL_BYTES && file.seek(SeekFrom::End(-(LOG_TAIL_BYTES as i64))).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|line| line.contains("ERROR") || line.to_ascii_lowercase().contains("panic"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn plugin_diagnostics(root: &Path) -> Vec<PluginDiagnostic> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    directories.sort();

    directories
        .into_iter()
        .map(|directory| {
            let manifest_path = directory.join("plugin.json");
            let manifest = std::fs::read(&manifest_path)
                .context("read plugin.json")
                .and_then(|bytes| {
                    serde_json::from_slice::<serde_json::Value>(&bytes).context("parse plugin.json")
                });
            let (id, version, package, error) = match manifest {
                Ok(manifest) => {
                    let id = manifest
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                    let version = manifest
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                    let file_name = manifest
                        .get("package")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("plugin.nshpkg");
                    let safe = steward_plugin_registry::manifest::is_safe_package_path(Path::new(
                        file_name,
                    ));
                    (
                        id,
                        version,
                        directory.join(if safe { file_name } else { "plugin.nshpkg" }),
                        (!safe).then(|| format!("unsafe package path '{file_name}'")),
                    )
                }
                Err(error) => (
                    None,
                    None,
                    directory.join("plugin.nshpkg"),
                    Some(format!("{error:#}")),
                ),
            };
            let package_size = package.metadata().ok().map(|metadata| metadata.len());
            let package_sha256 = package_size
                .filter(|size| *size <= MAX_HASHED_PACKAGE_BYTES)
                .and_then(|_| sha256_file(&package).ok());
            PluginDiagnostic {
                directory: directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                id,
                version,
                package_exists: package.is_file(),
                package,
                package_size,
                package_sha256,
                error,
            }
        })
        .collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).context("read file for hashing")?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(target_os = "windows")]
fn collect_os() -> OsDiagnostic {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    use windows_sys::Win32::UI::HiDpi::GetProcessDpiAwareness;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;

    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }

    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };
    let version_status = unsafe { RtlGetVersion(&mut info) };
    let mut awareness = -1_i32;
    let awareness_status = unsafe { GetProcessDpiAwareness(GetCurrentProcess(), &mut awareness) };
    let keyboard_layout = unsafe { GetKeyboardLayout(0) };

    OsDiagnostic {
        family: "windows".into(),
        architecture: std::env::consts::ARCH.into(),
        version: (version_status >= 0).then(|| format!("{}.{}", info.major, info.minor)),
        build: (version_status >= 0).then_some(info.build),
        process_dpi_awareness: (awareness_status >= 0).then(|| {
            match awareness {
                0 => "unaware",
                1 => "system",
                2 => "per_monitor",
                _ => "unknown",
            }
            .into()
        }),
        keyboard_layout: Some(format!("{keyboard_layout:p}")),
    }
}

#[cfg(not(target_os = "windows"))]
fn collect_os() -> OsDiagnostic {
    OsDiagnostic {
        family: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        ..Default::default()
    }
}

#[cfg(target_os = "windows")]
fn collect_displays() -> DisplayDiagnostic {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> i32 {
        let monitors = &mut *(data as *mut Vec<MonitorDiagnostic>);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return 1;
        }
        let mut dpi_x = 0;
        let mut dpi_y = 0;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) < 0 {
            dpi_x = 96;
            dpi_y = 96;
        }
        let dpi = dpi_x.max(dpi_y).max(96);
        monitors.push(MonitorDiagnostic {
            index: monitors.len() as u32,
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            bounds: rect(info.rcMonitor),
            work_area: rect(info.rcWork),
            dpi,
            scale_percent: ((dpi as u64 * 100) / 96) as u32,
        });
        1
    }

    fn rect(rect: RECT) -> PixelRect {
        PixelRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    let mut monitors: Vec<MonitorDiagnostic> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            (&mut monitors as *mut Vec<MonitorDiagnostic>) as LPARAM,
        )
    };
    let mixed_dpi = monitors
        .first()
        .is_some_and(|first| monitors.iter().any(|monitor| monitor.dpi != first.dpi));
    DisplayDiagnostic {
        monitor_count: if ok == 0 { 0 } else { monitors.len() },
        mixed_dpi,
        monitors,
    }
}

#[cfg(not(target_os = "windows"))]
fn collect_displays() -> DisplayDiagnostic {
    DisplayDiagnostic::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "steward-diagnostics-{name}-{}-{}",
            std::process::id(),
            unix_millis()
        ))
    }

    #[test]
    fn report_contains_schema_plugins_logs_and_recent_errors() {
        let root = scratch("report");
        let plugin = root.join("plugins/com.example.test");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            r#"{"schemaVersion":2,"id":"com.example.test","name":"Test","version":"1.2.3","package":"plugin.nshpkg","commands":[{"name":"run","title":"Run","trigger":{"type":"command"}}]}"#,
        )
        .unwrap();
        std::fs::write(plugin.join("plugin.nshpkg"), b"not a signed package").unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(
            root.join("logs/steward.log.2026-11-21"),
            "INFO healthy\nERROR plugin package failed verification\n",
        )
        .unwrap();
        let mut registry =
            steward_plugin_registry::Registry::open_with_root(&root, &root.join("plugins"))
                .unwrap();
        registry.scan().unwrap();
        drop(registry);

        let output = root.join("diagnostics/report.json");
        write_report(&root, Some(&output)).unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();

        assert_eq!(report["schemaVersion"], DIAGNOSTIC_SCHEMA_VERSION);
        assert_eq!(report["plugins"][0]["id"], "com.example.test");
        assert_eq!(report["plugins"][0]["packageExists"], true);
        assert_eq!(report["pluginCache"]["routeCount"], 1);
        assert_eq!(report["pluginCache"]["routes"][0]["id"], "com.example.test");
        assert_eq!(report["logFiles"].as_array().unwrap().len(), 1);
        assert!(report["recentErrors"][0]
            .as_str()
            .unwrap()
            .contains("failed verification"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn error_tail_ignores_healthy_lines_and_handles_crlf() {
        let root = scratch("tail");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("steward.log");
        std::fs::write(&path, "INFO ok\r\nERROR bad\r\napplication panic: boom\r\n").unwrap();

        assert_eq!(
            read_error_tail(&path),
            vec!["ERROR bad", "application panic: boom"]
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn plugin_scan_does_not_follow_an_unsafe_package_path() {
        let root = scratch("unsafe-package");
        let plugin = root.join("com.example.unsafe");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            r#"{"schemaVersion":2,"id":"com.example.unsafe","name":"Unsafe","version":"1.0.0","package":"../outside.nshpkg","commands":[{"name":"run","title":"Run","trigger":{"type":"command"}}]}"#,
        )
        .unwrap();
        std::fs::write(root.join("outside.nshpkg"), b"must not be hashed").unwrap();

        let plugin = plugin_diagnostics(&root);
        assert_eq!(plugin.len(), 1);
        assert_eq!(
            plugin[0].package,
            root.join("com.example.unsafe/plugin.nshpkg")
        );
        assert!(plugin[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsafe package path")));
        assert!(plugin[0].package_sha256.is_none());
        std::fs::remove_dir_all(root).ok();
    }
}
