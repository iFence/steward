use std::io::Write as _;
use std::path::{Path, PathBuf};

use nsh_package::{build as build_package, certify, Contents, SecretKey};
use steward_plugin_contract::HOST_API_VERSION;
use steward_plugin_store::{
    create_revocation_list, scaffold_plugin, PluginOperationKind, PluginStore, PluginStoreConfig,
    ScaffoldOptions,
};

struct Signer {
    root: SecretKey,
    release: SecretKey,
    certificate: nsh_package::Certificate,
    root_base64: String,
}

impl Signer {
    fn new() -> Self {
        let root = SecretKey::generate().unwrap();
        let release = SecretKey::generate().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let certificate = certify(
            &root,
            &release.public(),
            &["com.example.*".into()],
            now.saturating_sub(60),
            now + 3_600,
        );
        Self {
            root_base64: root.public().to_base64(),
            root,
            release,
            certificate,
        }
    }

    fn package(&self, version: &str, build: u64, capabilities: serde_json::Value) -> Vec<u8> {
        build_package(
            &Contents {
                app: nsh_package::App {
                    id: "com.example.test".into(),
                    name: "Test Plugin".into(),
                    version: version.into(),
                    build,
                },
                capabilities: Some(capabilities),
                source: "mount(() => null);".into(),
                bytecode: None,
                source_map: None,
                assets: Vec::new(),
                min_runtime: None,
            },
            &self.release,
            &self.certificate,
        )
        .unwrap()
    }

    fn zip(&self, directory: &Path, version: &str, build: u64) -> PathBuf {
        self.zip_with_capabilities(
            directory,
            version,
            build,
            serde_json::json!({ "host": ["steward.storage.get"] }),
        )
    }

    fn zip_with_capabilities(
        &self,
        directory: &Path,
        version: &str,
        build: u64,
        capabilities: serde_json::Value,
    ) -> PathBuf {
        self.zip_with_host_version(directory, version, build, capabilities, Some("0.1.0"))
    }

    fn zip_with_host_version(
        &self,
        directory: &Path,
        version: &str,
        build: u64,
        capabilities: serde_json::Value,
        min_host_version: Option<&str>,
    ) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let mut manifest = serde_json::json!({
            "schemaVersion": 2,
            "id": "com.example.test",
            "name": "Test Plugin",
            "version": version,
            "package": "plugin.nshpkg",
            "commands": [{
                "name": "test",
                "title": "Test Plugin",
                "trigger": { "type": "command" }
            }]
        });
        if let Some(min_host_version) = min_host_version {
            manifest["minHostVersion"] = serde_json::Value::String(min_host_version.into());
        }
        let path = directory.join(format!("test-{version}.zip"));
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.add_directory("test/", options).unwrap();
        zip.start_file("test/plugin.json", options).unwrap();
        zip.write_all(&serde_json::to_vec_pretty(&manifest).unwrap())
            .unwrap();
        zip.start_file("test/plugin.nshpkg", options).unwrap();
        zip.write_all(&self.package(version, build, capabilities))
            .unwrap();
        zip.finish().unwrap();
        path
    }
}

fn store(root: &Path, signer: &Signer) -> PluginStore {
    PluginStore::open(PluginStoreConfig::new(
        root.join("data"),
        root.join("data").join("plugins"),
        vec![signer.root_base64.clone()],
    ))
    .unwrap()
}

fn temp_root(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "steward-plugin-store-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn scaffold_writes_a_buildable_two_file_ready_plugin() {
    let root = temp_root("scaffold");
    let report = scaffold_plugin(ScaffoldOptions {
        id: "com.example.scaffold".into(),
        name: "Scaffold".into(),
        version: "0.1.0".into(),
        command: None,
        directory: root.join("com.example.scaffold"),
    })
    .unwrap();
    assert_eq!(report.plugin_id, "com.example.scaffold");
    for name in [
        "plugin.json",
        "nsh.json",
        "package.json",
        "tsconfig.json",
        "App.tsx",
        "README.md",
        ".gitignore",
    ] {
        assert!(report.directory.join(name).is_file(), "missing {name}");
    }
    let manifest: steward_plugin_registry::PluginManifest =
        serde_json::from_slice(&std::fs::read(report.directory.join("plugin.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.commands[0].name, "scaffold");
    assert_eq!(manifest.min_host_version.as_deref(), Some(HOST_API_VERSION));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn inspection_reports_signature_publisher_and_policy_review() {
    let root = temp_root("inspect");
    let signer = Signer::new();
    let zip = signer.zip_with_capabilities(
        &root,
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.net.fetch", "steward.exec.run"],
            "network": {
                "http": [{
                    "host": "api.example.com",
                    "schemes": ["https"],
                    "methods": ["GET"],
                    "pathPrefixes": ["/v1"]
                }],
                "sockets": ["example.com:9000"]
            },
            "process": {
                "execute": ["git.exe"],
                "env": ["SECRET_TOKEN"]
            }
        }),
    );
    let inspection = store(&root, &signer).inspect(&zip).unwrap();
    assert!(inspection.signature_valid);
    assert!(inspection.trusted);
    assert_eq!(
        inspection.publisher_key_id.as_deref(),
        Some(signer.release.public().id().as_str())
    );
    assert_eq!(inspection.app_id, "com.example.test");
    assert!(inspection.policy.passed);
    assert!(inspection
        .policy
        .denied_capabilities
        .contains(&"network".into()));
    assert!(inspection
        .policy
        .denied_capabilities
        .contains(&"process".into()));
    assert!(inspection
        .declared_capabilities
        .iter()
        .any(|capability| capability.permission == "steward.net.fetch"));
    assert!(inspection
        .declared_capabilities
        .iter()
        .any(|capability| capability.permission == "steward.exec.run"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn third_party_packages_require_a_host_version_and_named_host_permissions() {
    let root = temp_root("contract-policy");
    let signer = Signer::new();
    let zip = signer.zip_with_capabilities(
        &root,
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.*"] }),
    );
    let inspection = store(&root, &signer).inspect(&zip).unwrap();
    assert!(!inspection.installable());
    assert!(inspection
        .policy
        .errors
        .iter()
        .any(|error| error.contains("cannot declare wildcard host permissions")));

    let missing_version = signer.zip_with_host_version(
        &root,
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
        None,
    );
    let inspection = store(&root, &signer).inspect(&missing_version).unwrap();
    assert!(!inspection.installable());
    assert!(inspection
        .policy
        .errors
        .iter()
        .any(|error| error.contains("minHostVersion is required")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_admission_accepts_scoped_declarations_with_stable_evidence() {
    let root = temp_root("public-admission-pass");
    let signer = Signer::new();
    let zip = signer.zip_with_capabilities(
        &root,
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.net.fetch", "steward.exec.run", "steward.exec.cancel"],
            "network": {
                "http": [{
                    "host": "api.example.com",
                    "schemes": ["https"],
                    "methods": ["GET"],
                    "pathPrefixes": ["/v1"]
                }]
            },
            "process": { "execute": ["git.exe"] }
        }),
    );
    let review = store(&root, &signer).review(&zip).unwrap();
    assert!(review.accepted, "{:?}", review.checks);
    assert_eq!(review.policy_version, 1);
    assert!(review.checks.iter().all(|check| check.passed));
    assert!(review
        .capability_groups
        .iter()
        .any(|group| group.permission == "steward.net.fetch"));
    assert!(review
        .capability_groups
        .iter()
        .any(|group| group.permission == "steward.exec.run"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_admission_rejects_policy_warnings_and_overbroad_requests() {
    let root = temp_root("public-admission-reject");
    let signer = Signer::new();
    let zip = signer.zip_with_capabilities(
        &root,
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.net.fetch", "steward.exec.run"],
            "network": {
                "http": [{
                    "host": "api.example.com",
                    "schemes": ["https"],
                    "methods": ["GET"]
                }],
                "sockets": ["example.com:9000"]
            },
            "process": {
                "execute": ["git.exe"],
                "env": ["SECRET_TOKEN"]
            }
        }),
    );
    let review = store(&root, &signer).review(&zip).unwrap();
    assert!(!review.accepted);
    assert!(review
        .checks
        .iter()
        .any(|check| check.code == "capability_scope" && !check.passed));
    assert!(review.denied_capabilities.contains(&"network".into()));
    assert!(review.denied_capabilities.contains(&"process".into()));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn install_update_and_rollback_restore_verified_history() {
    let root = temp_root("lifecycle");
    let signer = Signer::new();
    let store = store(&root, &signer);
    let v1 = signer.zip(&root, "1.0.0", 1);
    let v2 = signer.zip(&root, "1.1.0", 2);

    assert!(store.update(&v1).is_err());
    let installed = store.install(&v1).unwrap();
    assert_eq!(installed.action, PluginOperationKind::Install);
    let updated = store.install(&v2).unwrap();
    assert_eq!(updated.action, PluginOperationKind::Update);
    assert_eq!(updated.previous_version.as_deref(), Some("1.0.0"));
    assert!(updated.added_capabilities.is_empty());
    assert!(!updated.consent_required);

    let history = store.history("com.example.test").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].version, "1.0.0");

    let rolled_back = store.rollback("com.example.test", None).unwrap();
    assert_eq!(rolled_back.action, PluginOperationKind::Rollback);
    assert_eq!(rolled_back.version, "1.0.0");
    let installed = store.list_installed();
    assert_eq!(installed[0].manifest.version, "1.0.0");
    assert_eq!(
        store.history("com.example.test").unwrap()[0].version,
        "1.1.0"
    );

    let removed = store.remove("com.example.test").unwrap();
    assert_eq!(removed.action, PluginOperationKind::Remove);
    assert!(store.list_installed().is_empty());
    assert_eq!(
        store.history("com.example.test").unwrap()[0].version,
        "1.0.0"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn updates_surface_new_sensitive_capabilities_for_renewed_consent() {
    let root = temp_root("capability-upgrade");
    let signer = Signer::new();
    let store = store(&root, &signer);
    let v1 = signer.zip_with_capabilities(
        &root,
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    let v2 = signer.zip_with_capabilities(
        &root,
        "1.1.0",
        2,
        serde_json::json!({
            "host": [
                "steward.storage.get",
                "steward.clipboard.write"
            ]
        }),
    );
    store.install(&v1).unwrap();
    let update = store.update(&v2).unwrap();
    assert!(update.consent_required);
    assert_eq!(
        update
            .added_capabilities
            .iter()
            .map(|capability| capability.permission.as_str())
            .collect::<Vec<_>>(),
        vec!["steward.clipboard.write"]
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn marketplace_indexes_and_installs_verified_local_archives() {
    let root = temp_root("marketplace");
    let signer = Signer::new();
    let store = store(&root, &signer);
    let zip = signer.zip(&root, "2.0.0", 2);
    let destination = store
        .marketplace_dir()
        .join("packages")
        .join(zip.file_name().unwrap());
    std::fs::copy(zip, destination).unwrap();

    let report = store.sync_marketplace().unwrap();
    assert_eq!(report.entries, 1);
    let index = store.marketplace().unwrap();
    assert_eq!(index.entries[0].id, "com.example.test");
    assert_eq!(index.entries[0].admission_policy_version, None);
    assert_eq!(index.publishers[0].key_id, signer.release.public().id());

    let installed = store.install_from_marketplace("com.example.test").unwrap();
    assert_eq!(installed.version, "2.0.0");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_marketplace_sync_rejects_packages_that_need_manual_policy_review() {
    let root = temp_root("marketplace-public-admission");
    let signer = Signer::new();
    let store = store(&root, &signer);
    let zip = signer.zip_with_capabilities(
        &root,
        "2.0.0",
        2,
        serde_json::json!({
            "host": ["steward.net.fetch"],
            "network": {
                "hosts": ["*"],
                "http": [{
                    "host": "api.example.com",
                    "schemes": ["https"],
                    "methods": ["GET"]
                }]
            }
        }),
    );
    let destination = store
        .marketplace_dir()
        .join("packages")
        .join(zip.file_name().unwrap());
    std::fs::copy(zip, destination).unwrap();
    let accepted = signer.zip_with_capabilities(
        &root,
        "2.1.0",
        3,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    let destination = store
        .marketplace_dir()
        .join("packages")
        .join(accepted.file_name().unwrap());
    std::fs::copy(accepted, destination).unwrap();

    let public = store.sync_public_marketplace().unwrap();
    assert!(public.public_admission);
    assert_eq!(public.entries, 1);
    assert!(public
        .errors
        .iter()
        .any(|error| error.contains("public admission rejected")));
    let public_index = store.marketplace().unwrap();
    assert_eq!(public_index.entries[0].version, "2.1.0");
    assert_eq!(public_index.entries[0].admission_policy_version, Some(1));
    assert!(!public_index.entries[0].review_required);

    let local = store.sync_marketplace().unwrap();
    assert!(!local.public_admission);
    assert_eq!(local.entries, 2);
    assert!(store
        .marketplace()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.version == "2.0.0" && entry.review_required));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn applied_revocation_rejects_the_release_key() {
    let root = temp_root("revocation");
    let signer = Signer::new();
    let store = store(&root, &signer);
    let zip = signer.zip(&root, "1.0.0", 1);
    let root_key = root.join("root.secret");
    std::fs::write(&root_key, signer.root.to_base64()).unwrap();
    let revocation = root.join("revocations.json");
    create_revocation_list(
        &root_key,
        None,
        1,
        &[signer.release.public().id()],
        &[],
        &revocation,
    )
    .unwrap();
    let applied = store.apply_revocations(&revocation).unwrap();
    assert_eq!(applied.sequence, 1);

    let inspection = store.inspect(&zip).unwrap();
    assert!(inspection.signature_valid);
    assert!(!inspection.trusted);
    assert!(inspection
        .trust_error
        .as_deref()
        .is_some_and(|error| error.contains("revoked")));
    let review = store.review(&zip).unwrap();
    assert!(!review.accepted);
    assert!(review
        .checks
        .iter()
        .any(|check| check.code == "publisher_identity" && !check.passed));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn archive_contract_rejects_extra_files() {
    let root = temp_root("archive-contract");
    let file = std::fs::File::create(root.join("bad.zip")).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    zip.add_directory("test/", options).unwrap();
    zip.start_file("test/plugin.json", options).unwrap();
    zip.write_all(b"{}").unwrap();
    zip.start_file("test/extra.txt", options).unwrap();
    zip.write_all(b"unexpected").unwrap();
    zip.finish().unwrap();
    let signer = Signer::new();
    let error = store(&root, &signer)
        .inspect(&root.join("bad.zip"))
        .unwrap_err();
    assert!(format!("{error:#}").contains("unsupported file"));
    std::fs::remove_dir_all(root).ok();
}
