//! `steward-index-helper`: the optional privileged enumerator.
//!
//! Modes:
//! - `--service` (what the MSI installs): connect to the SCM and serve the pipe
//!   as LocalSystem, which is what makes the `$MFT` fast path available.
//! - no flag: serve the pipe in the foreground, as the current user (useful for
//!   development and for the walk-only path).
//! - `--install-service` / `--uninstall-service`: register or remove the
//!   service with `sc.exe` (requires an elevated prompt).
//! - `--pipe \\.\pipe\name`: use a custom pipe name (tests and development).

fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let mut pipe = steward_index_helper::PIPE_NAME.to_string();
        let mut service = false;
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--pipe" => {
                    pipe = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--pipe needs a value"))?;
                }
                "--service" => service = true,
                "--install-service" => return install_service(),
                "--uninstall-service" => return uninstall_service(),
                "--help" | "-h" => {
                    println!(
                        "usage: steward-index-helper [--pipe \\\\.\\pipe\\name] [--service]\n\
                         \x20      steward-index-helper --install-service | --uninstall-service"
                    );
                    return Ok(());
                }
                other => anyhow::bail!("unknown argument {other}"),
            }
        }
        if service {
            steward_index_helper::service::run()?;
        } else {
            eprintln!("steward-index-helper: listening on {pipe}");
            steward_index_helper::server::serve(&pipe)?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("steward-index-helper requires Windows");
    }
}

/// Register the auto-start LocalSystem service (needs an elevated prompt).
#[cfg(target_os = "windows")]
fn install_service() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let bin_path = format!("\"{}\" --service", exe.display());
    run_sc(&[
        "create",
        steward_index_helper::service::SERVICE_NAME,
        "binPath=",
        &bin_path,
        "start=",
        "auto",
        "DisplayName=",
        "Steward Index Helper",
    ])?;
    run_sc(&["start", steward_index_helper::service::SERVICE_NAME])?;
    Ok(())
}

/// Stop and remove the service (needs an elevated prompt).
#[cfg(target_os = "windows")]
fn uninstall_service() -> anyhow::Result<()> {
    // Stopping a service that is not running reports an error; that is fine.
    let _ = std::process::Command::new("sc.exe")
        .args(["stop", steward_index_helper::service::SERVICE_NAME])
        .status();
    run_sc(&["delete", steward_index_helper::service::SERVICE_NAME])?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_sc(args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("sc.exe").args(args).status()?;
    anyhow::ensure!(
        status.success(),
        "sc.exe {} failed ({status}); run this from an elevated prompt",
        args.first().copied().unwrap_or("")
    );
    Ok(())
}
