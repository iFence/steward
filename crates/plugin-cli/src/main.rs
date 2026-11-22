use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};
use steward_plugin_store::{
    create_revocation_list, default_developer_data_dir, package_development_project,
    request_development_reload, scaffold_plugin, DevelopmentReloadRequest, PluginOperationKind,
    PluginStore, PluginStoreConfig, ScaffoldOptions,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("steward-plugin: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }
    let developer_mode = take_flag(&mut args, "--developer-mode");
    let developer_data_dir = take_path_option(&mut args, "--developer-data-dir")?;
    let developer_plugins_dir = take_path_option(&mut args, "--developer-plugins-dir")?;
    let data_dir_option = take_path_option(&mut args, "--data-dir")?;
    let plugins_dir_override = take_path_option(&mut args, "--plugins-dir")?;
    if developer_mode && (data_dir_option.is_some() || plugins_dir_override.is_some()) {
        anyhow::bail!(
            "developer mode uses --developer-data-dir/--developer-plugins-dir and never targets the installed profile"
        );
    }
    if !developer_mode && (developer_data_dir.is_some() || developer_plugins_dir.is_some()) {
        anyhow::bail!("--developer-data-dir and --developer-plugins-dir require --developer-mode");
    }
    let data_dir = if developer_mode {
        developer_data_dir.unwrap_or(default_developer_data_dir()?)
    } else {
        data_dir_option.unwrap_or_else(default_data_dir)
    };
    let mut roots = default_trust_roots()?;
    while let Some(root) = take_value_option(&mut args, "--trust-root")? {
        roots.push(root);
    }
    if args.is_empty() {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    if command == "new" {
        return scaffold(&args);
    }
    let plugins_dir = if developer_mode {
        developer_plugins_dir.unwrap_or_else(|| data_dir.join("plugins"))
    } else {
        plugins_dir_override.unwrap_or_else(|| data_dir.join("plugins"))
    };
    let store = PluginStore::open(
        PluginStoreConfig::new(data_dir, plugins_dir, roots).with_developer_mode(developer_mode),
    )?;
    match command.as_str() {
        "inspect" => inspect(&store, &args),
        "check" => check(&store, &args),
        "doctor" => doctor(&store, &args),
        "dev" => develop(&store, &args),
        "review" => review(&store, &args),
        "list" => list(&store, &args),
        "install" => install(&store, &args, false),
        "update" => install(&store, &args, true),
        "remove" => remove(&store, &args),
        "rollback" => rollback(&store, &args),
        "marketplace" => marketplace(&store, &args),
        "publisher" => publisher(&store, &args),
        "revoke" => revoke(&store, &args),
        other => anyhow::bail!("unknown command '{other}'; run `steward-plugin help`"),
    }
}

fn scaffold(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let directory = positional(&mut args, "scaffold target")?;
    let id = required_option(&mut args, "--id")?;
    let name = required_option(&mut args, "--name")?;
    let version = take_value_option(&mut args, "--version")?.unwrap_or_else(|| "0.1.0".into());
    let command = take_value_option(&mut args, "--command")?;
    let sdk_dependency = take_value_option(&mut args, "--sdk")?;
    reject_extra(&args)?;
    let report = scaffold_plugin(ScaffoldOptions {
        id,
        name,
        version,
        command,
        sdk_dependency,
        directory: PathBuf::from(directory),
    })?;
    println!(
        "Created plugin {} at {}",
        report.plugin_id,
        report.directory.display()
    );
    for file in report.files {
        println!("  {}", file.display());
    }
    Ok(())
}

fn inspect(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let path = positional(&mut args, "plugin source")?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let inspection = store.inspect(Path::new(&path))?;
    if json {
        print_json(&inspection)?;
    } else {
        print_inspection(&inspection);
    }
    Ok(())
}

fn check(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let path = positional(&mut args, "plugin source")?;
    let json = take_flag(&mut args, "--json");
    let strict = take_flag(&mut args, "--strict");
    reject_extra(&args)?;
    let inspection = store.inspect(Path::new(&path))?;
    if json {
        print_json(&inspection)?;
    } else {
        print_inspection(&inspection);
    }
    if !inspection.installable() {
        anyhow::bail!("package failed installability checks");
    }
    if strict && !inspection.policy.warnings.is_empty() {
        anyhow::bail!("package failed strict policy review");
    }
    Ok(())
}

fn doctor(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let target = positional(&mut args, "plugin id, zip, or package directory")?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let report = store.doctor(&target);
    if json {
        print_json(&report)?;
    } else {
        println!(
            "{}: {}",
            report.target,
            if report.passed { "ready" } else { "failed" }
        );
        if let Some(path) = &report.resolved_path {
            println!("  source: {}", path.display());
        }
        for check in &report.checks {
            println!(
                "  [{}] {:<14} {} - {}",
                if check.passed { "pass" } else { "fail" },
                format!("{:?}", check.stage).to_ascii_lowercase(),
                check.id,
                check.detail
            );
        }
    }
    if !report.passed {
        anyhow::bail!(
            "plugin failed launch doctor at stage {}",
            report
                .stage
                .map(|stage| format!("{stage:?}").to_ascii_lowercase())
                .unwrap_or_else(|| "unknown".into())
        );
    }
    Ok(())
}

fn develop(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let project = PathBuf::from(positional(&mut args, "plugin project directory")?);
    let build_command = take_value_option(&mut args, "--build-command")?;
    let package = take_path_option(&mut args, "--package")?
        .unwrap_or_else(|| PathBuf::from("dist/plugin.nshpkg"));
    let entry = take_path_option(&mut args, "--entry")?.unwrap_or_else(|| PathBuf::from("App.tsx"));
    let signing_key = take_path_option(&mut args, "--signing-key")?
        .or_else(|| env_path("STEWARD_NSH_RELEASE_KEY"));
    let signing_cert = take_path_option(&mut args, "--signing-cert")?
        .or_else(|| env_path("STEWARD_NSH_RELEASE_CERT"));
    let skip_build = take_flag(&mut args, "--no-build");
    let no_open = take_flag(&mut args, "--no-open");
    let reload = take_value_option(&mut args, "--reload")?.unwrap_or_else(|| "static".into());
    reject_extra(&args)?;
    if reload != "static" {
        anyhow::bail!(
            "unsupported reload mode '{reload}'; PDX-2 currently uses the static-bundle fallback"
        );
    }
    if !project.is_dir() {
        anyhow::bail!("plugin project is not a directory: {}", project.display());
    }

    if !skip_build {
        run_project_build(&project, build_command.as_deref())?;
    }
    let package_path = project.join(&package);
    let should_sign = !skip_build && signing_key.is_some() && signing_cert.is_some();
    if should_sign {
        let key = signing_key.as_deref().expect("signing key checked above");
        let cert = signing_cert
            .as_deref()
            .expect("signing certificate checked above");
        if let Some(parent) = package_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let staged_package =
            package_path.with_extension(format!("development-{}.nshpkg", std::process::id()));
        let _ = std::fs::remove_file(&staged_package);
        let status = Command::new("nsh")
            .arg("publish")
            .arg(&entry)
            .arg("--key")
            .arg(key)
            .arg("--cert")
            .arg(cert)
            .arg("-o")
            .arg(&staged_package)
            .current_dir(&project)
            .status()
            .context("run `nsh publish`")?;
        if !status.success() {
            let _ = std::fs::remove_file(&staged_package);
            anyhow::bail!("`nsh publish` failed with {status}");
        }
        if package_path.exists() {
            std::fs::remove_file(&package_path)
                .with_context(|| format!("replace signed package {}", package_path.display()))?;
        }
        std::fs::rename(&staged_package, &package_path)
            .with_context(|| format!("activate signed package {}", package_path.display()))?;
    } else if !package_path.is_file() {
        anyhow::bail!(
            "signed package is missing; pass --signing-key/--signing-cert or set STEWARD_NSH_RELEASE_KEY/STEWARD_NSH_RELEASE_CERT"
        );
    } else if !skip_build && source_is_newer(&project, &package_path) {
        anyhow::bail!(
            "signed package is older than the edited plugin source; configure signing keys or make the build command emit a fresh package"
        );
    }

    let archive = store.data_dir().join(".development-packages").join(format!(
        "plugin-{}-{}.zip",
        std::process::id(),
        unix_millis()
    ));
    package_development_project(&project, &package, &archive)?;
    let inspection = store.inspect(&archive)?;
    let command = inspection
        .manifest
        .commands
        .first()
        .map(|command| command.name.clone())
        .context("plugin manifest must contain a launch command")?;
    let title = inspection.manifest.name.clone();
    let operation = store.replace_development(&archive)?;
    let _ = std::fs::remove_file(&archive);

    let reload_path = request_development_reload(
        store.data_dir(),
        &DevelopmentReloadRequest {
            plugin_id: operation.plugin_id.clone(),
            command,
            title,
            open: !no_open,
            requested_at: unix_millis(),
        },
    )?;
    print_operation(&operation);
    println!("  static reload queued: {}", reload_path.display());
    Ok(())
}

fn run_project_build(project: &Path, command: Option<&str>) -> Result<()> {
    let command = match command {
        Some(command) => command.to_owned(),
        None => default_project_build(project)?,
    };
    let status = if cfg!(target_os = "windows") {
        Command::new("cmd.exe")
            .args(["/d", "/s", "/c", &command])
            .current_dir(project)
            .status()
    } else {
        Command::new("sh")
            .args(["-cu", &command])
            .current_dir(project)
            .status()
    }
    .with_context(|| format!("run plugin development build `{command}`"))?;
    if !status.success() {
        anyhow::bail!("plugin development build failed with {status}");
    }
    Ok(())
}

fn default_project_build(project: &Path) -> Result<String> {
    let package_json = project.join("package.json");
    if let Ok(bytes) = std::fs::read(&package_json) {
        let package: serde_json::Value =
            serde_json::from_slice(&bytes).context("parse package.json")?;
        if package
            .get("scripts")
            .and_then(|scripts| scripts.get("dev:build"))
            .is_some()
        {
            return Ok("corepack pnpm run dev:build".into());
        }
        if package
            .get("scripts")
            .and_then(|scripts| scripts.get("build"))
            .is_some()
        {
            return Ok("corepack pnpm run build".into());
        }
    }
    Ok("nsh build App.tsx --release -o dist".into())
}

fn source_is_newer(project: &Path, package: &Path) -> bool {
    let Ok(package_modified) = std::fs::metadata(package).and_then(|metadata| metadata.modified())
    else {
        return true;
    };
    let mut pending = vec![project.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                if !matches!(
                    name.to_str().unwrap_or_default(),
                    "dist" | "node_modules" | ".git" | ".nsh" | "target"
                ) {
                    pending.push(path);
                }
                continue;
            }
            if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "ts" | "tsx" | "js" | "jsx" | "json" | "css" | "svg"
                        )
                    })
                && std::fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified > package_modified)
            {
                return true;
            }
        }
    }
    false
}

fn review(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let path = positional(&mut args, "plugin source")?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let review = store.review(Path::new(&path))?;
    if json {
        print_json(&review)?;
    } else {
        print_admission_review(&review);
    }
    if !review.accepted {
        anyhow::bail!("package failed public admission policy");
    }
    Ok(())
}

fn list(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    if json {
        print_json(&store.catalog())?;
        return Ok(());
    }
    let catalog = store.catalog();
    if catalog.installed.is_empty() {
        println!("No plugins installed.");
    }
    for plugin in &catalog.installed {
        let status = plugin
            .inspection
            .as_ref()
            .map(|inspection| {
                if inspection.installable() {
                    "trusted"
                } else if inspection.signature_valid {
                    "signature valid, trust failed"
                } else {
                    "invalid signature"
                }
            })
            .unwrap_or("inspection failed");
        println!(
            "{} {} ({}) - {}",
            plugin.manifest.name, plugin.manifest.version, plugin.manifest.id, status
        );
        if let Some(error) = &plugin.error {
            println!("  error: {error}");
        }
        if let Some(key_id) = plugin
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.publisher_key_id.as_deref())
        {
            println!("  publisher: {key_id}");
        }
        if !plugin.history.is_empty() {
            println!(
                "  rollback: {}",
                plugin
                    .history
                    .iter()
                    .map(|version| version.version.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    if !catalog.marketplace.entries.is_empty() {
        println!(
            "Local marketplace: {} package(s)",
            catalog.marketplace.entries.len()
        );
    }
    for error in &catalog.marketplace_errors {
        eprintln!("marketplace error: {error}");
    }
    Ok(())
}

fn install(store: &PluginStore, args: &[String], update: bool) -> Result<()> {
    let mut args = args.to_vec();
    let path = positional(
        &mut args,
        if update {
            "plugin update source"
        } else {
            "plugin install source"
        },
    )?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let operation = if update {
        store.update(Path::new(&path))?
    } else {
        store.install(Path::new(&path))?
    };
    if json {
        print_json(&operation)?;
    } else {
        print_operation(&operation);
    }
    Ok(())
}

fn remove(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let plugin_id = positional(&mut args, "plugin id")?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let operation = store.remove(&plugin_id)?;
    if json {
        print_json(&operation)?;
    } else {
        print_operation(&operation);
    }
    Ok(())
}

fn rollback(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let plugin_id = positional(&mut args, "plugin id")?;
    let version = take_value_option(&mut args, "--version")?;
    let json = take_flag(&mut args, "--json");
    reject_extra(&args)?;
    let operation = store.rollback(&plugin_id, version.as_deref())?;
    if json {
        print_json(&operation)?;
    } else {
        print_operation(&operation);
    }
    Ok(())
}

fn marketplace(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let action = positional(&mut args, "marketplace action")?;
    match action.as_str() {
        "sync" => {
            let json = take_flag(&mut args, "--json");
            let public_admission = take_flag(&mut args, "--public");
            reject_extra(&args)?;
            let report = if public_admission {
                store.sync_public_marketplace()?
            } else {
                store.sync_marketplace()?
            };
            if json {
                print_json(&report)?;
            } else {
                println!(
                    "Indexed {} package(s) from {} archive(s) into {} under {} policy",
                    report.entries,
                    report.packages_scanned,
                    report.index_path.display(),
                    if report.public_admission {
                        "public-admission"
                    } else {
                        "local-review"
                    }
                );
                for error in report.errors {
                    eprintln!("skipped: {error}");
                }
            }
        }
        "list" => {
            let json = take_flag(&mut args, "--json");
            reject_extra(&args)?;
            let index = store.marketplace()?;
            if json {
                print_json(&index)?;
            } else if index.entries.is_empty() {
                println!("Local marketplace is empty.");
            } else {
                for entry in index.entries {
                    let policy = entry
                        .admission_policy_version
                        .map(|version| format!(" public-policy-v{version}"))
                        .unwrap_or_default();
                    println!(
                        "{} {} ({}) publisher {} artifact {}{}",
                        entry.name,
                        entry.version,
                        entry.id,
                        entry.publisher_key_id,
                        entry.artifact,
                        policy
                    );
                }
            }
        }
        "install" | "update" => {
            let plugin_id = positional(&mut args, "plugin id")?;
            let json = take_flag(&mut args, "--json");
            reject_extra(&args)?;
            let operation = if action == "update" {
                store.update_from_marketplace(&plugin_id)?
            } else {
                store.install_from_marketplace(&plugin_id)?
            };
            if json {
                print_json(&operation)?;
            } else {
                print_operation(&operation);
            }
        }
        other => anyhow::bail!("unknown marketplace action '{other}'"),
    }
    Ok(())
}

fn publisher(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let action = positional(&mut args, "publisher action")?;
    match action.as_str() {
        "list" => {
            let json = take_flag(&mut args, "--json");
            reject_extra(&args)?;
            let index = store.marketplace()?;
            if json {
                print_json(&index.publishers)?;
            } else if index.publishers.is_empty() {
                println!("No marketplace publisher identities.");
            } else {
                for publisher in index.publishers {
                    println!(
                        "{} ({} package(s))",
                        publisher.key_id, publisher.package_count
                    );
                }
            }
        }
        "inspect" => inspect(store, &args)?,
        other => anyhow::bail!("unknown publisher action '{other}'"),
    }
    Ok(())
}

fn revoke(store: &PluginStore, args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let action = positional(&mut args, "revoke action")?;
    match action.as_str() {
        "apply" => {
            let path = positional(&mut args, "revocation list")?;
            let json = take_flag(&mut args, "--json");
            reject_extra(&args)?;
            let applied = store.apply_revocations(Path::new(&path))?;
            if json {
                print_json(&applied)?;
            } else {
                println!(
                    "Applied revocation sequence {} ({} key(s), {} package(s))",
                    applied.sequence,
                    applied.keys.len(),
                    applied.packages.len()
                );
            }
        }
        "create" => {
            let output = positional(&mut args, "revocation output")?;
            let root_key = required_option(&mut args, "--root-key")?;
            let sequence = required_option(&mut args, "--sequence")?
                .parse::<u64>()
                .context("parse --sequence")?;
            let previous = take_value_option(&mut args, "--previous")?;
            let mut keys = Vec::new();
            let mut packages = Vec::new();
            while let Some(key) = take_value_option(&mut args, "--key-id")? {
                keys.push(key);
            }
            while let Some(package) = take_value_option(&mut args, "--package-sha256")? {
                packages.push(package);
            }
            reject_extra(&args)?;
            let created = create_revocation_list(
                Path::new(&root_key),
                previous.as_deref().map(Path::new),
                sequence,
                &keys,
                &packages,
                Path::new(&output),
            )?;
            println!(
                "Wrote revocation sequence {} signed by root {} to {}",
                created.sequence,
                created.root_key_id,
                created.path.display()
            );
        }
        other => anyhow::bail!("unknown revoke action '{other}'"),
    }
    Ok(())
}

fn print_inspection(inspection: &steward_plugin_store::PackageInspection) {
    println!("{} {}", inspection.name, inspection.version);
    println!("  id: {}", inspection.app_id);
    println!("  build: {}", inspection.build);
    println!("  package sha256: {}", inspection.package_sha256);
    println!(
        "  publisher key id: {}",
        inspection.publisher_key_id.as_deref().unwrap_or("unknown")
    );
    println!(
        "  signature: {}",
        if inspection.signature_valid {
            "valid"
        } else {
            "invalid"
        }
    );
    println!(
        "  trust: {}",
        if inspection.trusted {
            "trusted"
        } else {
            "not trusted"
        }
    );
    if let Some(error) = &inspection.signature_error {
        println!("  signature error: {error}");
    }
    if let Some(error) = &inspection.trust_error {
        println!("  trust error: {error}");
    }
    if let Some(runtime) = &inspection.min_runtime {
        println!("  minimum runtime: {runtime}");
    }
    println!(
        "  minimum host API: {}",
        inspection.min_host_version.as_deref().unwrap_or("legacy")
    );
    if inspection.legacy_compatibility {
        println!("  compatibility: legacy official plugin");
    }
    println!(
        "  declared capabilities: {}",
        inspection.declared_capabilities.len()
    );
    for capability in &inspection.declared_capabilities {
        println!(
            "    {} [{}] - {}",
            capability.permission, capability.scope, capability.description
        );
    }
    println!(
        "  policy: {}",
        if inspection.policy.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for error in &inspection.policy.errors {
        println!("  policy error: {error}");
    }
    for warning in &inspection.policy.warnings {
        println!("  policy warning: {warning}");
    }
}

fn print_admission_review(review: &steward_plugin_store::AdmissionReview) {
    println!("{} {}", review.name, review.version);
    println!("  id: {}", review.plugin_id);
    println!("  build: {}", review.build);
    println!(
        "  publisher key id: {}",
        review.publisher_key_id.as_deref().unwrap_or("unknown")
    );
    println!(
        "  host API: {} (plugin requires {})",
        review.host_api_version,
        review.required_host_api.as_deref().unwrap_or("unspecified")
    );
    println!(
        "  admission: {} (policy v{})",
        if review.accepted {
            "accepted"
        } else {
            "rejected"
        },
        review.policy_version
    );
    println!("  capability groups:");
    if review.capability_groups.is_empty() {
        println!("    none");
    }
    for group in &review.capability_groups {
        println!(
            "    {} ({})",
            group.permission,
            if group.sensitive {
                "user grant required"
            } else {
                "automatic"
            }
        );
        for scope in &group.scopes {
            println!("      {scope}");
        }
    }
    if !review.denied_capabilities.is_empty() {
        println!(
            "  denied capability categories: {}",
            review.denied_capabilities.join(", ")
        );
    }
    println!("  checks:");
    for check in &review.checks {
        println!(
            "    [{}] {} - {}",
            if check.passed { "pass" } else { "fail" },
            check.code,
            check.detail
        );
    }
}

fn print_operation(operation: &steward_plugin_store::PluginOperation) {
    let action = match operation.action {
        PluginOperationKind::Install => "Installed",
        PluginOperationKind::Update => "Updated",
        PluginOperationKind::Remove => "Removed",
        PluginOperationKind::Rollback => "Rolled back",
    };
    println!(
        "{action} {} {} ({})",
        operation.name, operation.version, operation.plugin_id
    );
    println!("  publisher: {}", operation.publisher_key_id);
    println!("  package sha256: {}", operation.package_sha256);
    if let Some(previous) = &operation.previous_version {
        println!("  previous version: {previous}");
    }
    for capability in &operation.added_capabilities {
        println!(
            "  new permission: {} ({})",
            capability.permission, capability.description
        );
    }
    if operation.consent_required {
        println!("  consent: new sensitive capability requires approval");
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("serialize command output")?
    );
    Ok(())
}

fn positional(args: &mut Vec<String>, name: &str) -> Result<String> {
    if args.is_empty() {
        anyhow::bail!("missing {name}");
    }
    Ok(args.remove(0))
}

fn required_option(args: &mut Vec<String>, name: &str) -> Result<String> {
    take_value_option(args, name)?.with_context(|| format!("missing {name}"))
}

fn take_path_option(args: &mut Vec<String>, name: &str) -> Result<Option<PathBuf>> {
    Ok(take_value_option(args, name)?.map(PathBuf::from))
}

fn take_value_option(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    let Some(index) = args.iter().position(|value| value == name) else {
        return Ok(None);
    };
    if index + 1 >= args.len() {
        anyhow::bail!("{name} requires a value");
    }
    let value = args.remove(index + 1);
    args.remove(index);
    Ok(Some(value))
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|value| value == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn reject_extra(args: &[String]) -> Result<()> {
    if let Some(extra) = args.first() {
        anyhow::bail!("unexpected argument '{extra}'");
    }
    Ok(())
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("STEWARD_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no OS data directory available")
                .join("Steward")
        })
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn default_trust_roots() -> Result<Vec<String>> {
    let mut roots =
        serde_json::from_str::<Vec<String>>(include_str!("../../../assets/nsh/trust-roots.json"))
            .context("parse committed NSH trust roots")?;
    if let Ok(root) = std::env::var("STEWARD_NSH_TRUST_ROOT") {
        roots.push(root);
    }
    Ok(roots)
}

fn print_help() {
    println!(
        r#"steward-plugin - Steward plugin development and lifecycle CLI

Global options (before the command):
  --developer-mode      Use the isolated Steward-Developer profile
  --developer-data-dir <path>
                        Override the isolated developer data directory
  --developer-plugins-dir <path>
                        Override the isolated developer plugin directory
  --data-dir <path>     Steward data directory
  --plugins-dir <path>  Installed plugin directory
  --trust-root <key>    Additional base64 Ed25519 trust root

Commands:
  steward-plugin new <dir> --id <id> --name <name> [--version <semver>]
      [--command <name>] [--sdk <released-package-or-tarball>]
  steward-plugin inspect <zip-or-dir> [--json]
  steward-plugin check <zip-or-dir> [--strict] [--json]
  steward-plugin doctor <plugin-id-or-source> [--json]
  steward-plugin dev <project-dir> [--build-command <command>]
      [--package <relative-path>] [--entry <relative-path>]
      [--signing-key <path>] [--signing-cert <path>]
      [--no-build] [--no-open] [--reload static]
  steward-plugin review <zip-or-dir> [--json]
  steward-plugin list [--json]
  steward-plugin install <zip-or-dir> [--json]
  steward-plugin update <zip-or-dir> [--json]
  steward-plugin remove <plugin-id> [--json]
  steward-plugin rollback <plugin-id> [--version <semver>] [--json]
  steward-plugin marketplace sync [--public] [--json]
  steward-plugin marketplace list [--json]
  steward-plugin marketplace install <plugin-id> [--json]
  steward-plugin marketplace update <plugin-id> [--json]
  steward-plugin publisher list [--json]
  steward-plugin publisher inspect <zip-or-dir> [--json]
  steward-plugin revoke apply <revocations.json> [--json]
  steward-plugin revoke create <output.json> --root-key <path> --sequence <n>
      [--previous <path>] [--key-id <id>] [--package-sha256 <hex>]

Local marketplace packages are placed in
<data-dir>/marketplace/packages/*.zip and indexed with `marketplace sync`.
Use `marketplace sync --public` to apply the stricter public-admission policy.
"#
    );
}
