use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use nsh_package::{build as build_package, certify, Contents, SecretKey};
use steward_plugin_contract::HOST_API_VERSION;
use steward_plugin_store::{
    create_revocation_list, scaffold_plugin, PluginOperationKind, PluginStore, PluginStoreConfig,
    PublisherKeyStatus, PublisherRegistrationRequest, PublisherStatus, QuarantineRequest,
    QuarantineStatus, QuarantineTarget, ScaffoldOptions,
};

struct Signer {
    root: SecretKey,
    release: SecretKey,
    certificate: nsh_package::Certificate,
    root_base64: String,
    plugin_id: String,
    certificate_pattern: String,
}

impl Signer {
    fn new() -> Self {
        Self::with_identity("com.example.test", "com.example.*")
    }

    fn with_identity(plugin_id: &str, certificate_pattern: &str) -> Self {
        let root = SecretKey::generate().unwrap();
        let release = SecretKey::generate().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let certificate = certify(
            &root,
            &release.public(),
            &[certificate_pattern.into()],
            now.saturating_sub(60),
            now + 3_600,
        );
        Self {
            root_base64: root.public().to_base64(),
            root,
            release,
            certificate,
            plugin_id: plugin_id.into(),
            certificate_pattern: certificate_pattern.into(),
        }
    }

    fn package_with_release(
        &self,
        release: &SecretKey,
        certificate: &nsh_package::Certificate,
        version: &str,
        build: u64,
        capabilities: serde_json::Value,
    ) -> Vec<u8> {
        build_package(
            &Contents {
                app: nsh_package::App {
                    id: self.plugin_id.clone(),
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
            release,
            certificate,
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
        self.zip_with_host_version_and_release(
            directory,
            version,
            build,
            capabilities,
            min_host_version,
            (&self.release, &self.certificate),
        )
    }

    fn zip_with_host_version_and_release(
        &self,
        directory: &Path,
        version: &str,
        build: u64,
        capabilities: serde_json::Value,
        min_host_version: Option<&str>,
        signing: (&SecretKey, &nsh_package::Certificate),
    ) -> PathBuf {
        let (release, certificate) = signing;
        std::fs::create_dir_all(directory).unwrap();
        let mut manifest = serde_json::json!({
            "schemaVersion": 2,
            "id": self.plugin_id,
            "name": "Test Plugin",
            "author": "Test Author",
            "version": version,
            "icon": "<svg viewBox=\"0 0 16 16\"><circle cx=\"8\" cy=\"8\" r=\"8\"/></svg>",
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
        zip.add_directory(format!("{}/", self.plugin_id), options)
            .unwrap();
        zip.start_file(format!("{}/plugin.json", self.plugin_id), options)
            .unwrap();
        zip.write_all(&serde_json::to_vec_pretty(&manifest).unwrap())
            .unwrap();
        zip.start_file(format!("{}/plugin.nshpkg", self.plugin_id), options)
            .unwrap();
        zip.write_all(&self.package_with_release(
            release,
            certificate,
            version,
            build,
            capabilities,
        ))
        .unwrap();
        zip.finish().unwrap();
        path
    }

    fn rotated_zip(
        &self,
        directory: &Path,
        version: &str,
        build: u64,
        capabilities: serde_json::Value,
    ) -> (PathBuf, SecretKey) {
        let release = SecretKey::generate().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let certificate = certify(
            &self.root,
            &release.public(),
            std::slice::from_ref(&self.certificate_pattern),
            now.saturating_sub(60),
            now + 3_600,
        );
        let path = self.zip_with_host_version_and_release(
            directory,
            version,
            build,
            capabilities,
            Some("0.1.0"),
            (&release, &certificate),
        );
        (path, release)
    }
}

fn store(root: &Path, signer: &Signer) -> PluginStore {
    store_with_roots(root, std::slice::from_ref(&signer.root_base64))
}

fn store_with_roots(root: &Path, roots: &[String]) -> PluginStore {
    PluginStore::open(PluginStoreConfig::new(
        root.join("data"),
        root.join("data").join("plugins"),
        roots.to_vec(),
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

fn register_signer(
    store: &PluginStore,
    signer: &Signer,
    publisher_id: &str,
    domain: &str,
    package: &Path,
) -> steward_plugin_store::PublisherRecord {
    store
        .register_publisher(
            PublisherRegistrationRequest::new(
                publisher_id,
                publisher_id,
                format!("{publisher_id}@example.com"),
                vec![domain.into()],
            ),
            package,
            "pdx4-test-operator",
        )
        .unwrap_or_else(|error| {
            panic!(
                "registering {} key {} failed: {error:#}",
                publisher_id,
                signer.release.public().id()
            )
        })
}

#[test]
fn scaffold_writes_a_buildable_two_file_ready_plugin() {
    let root = temp_root("scaffold");
    let report = scaffold_plugin(ScaffoldOptions {
        id: "com.example.scaffold".into(),
        name: "Scaffold".into(),
        version: "0.1.0".into(),
        command: None,
        sdk_dependency: Some("https://plugins.example/steward-extension-api-0.5.0.tgz".into()),
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
        "App.test.tsx",
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
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report.directory.join("package.json")).unwrap())
            .unwrap();
    assert_eq!(
        package["dependencies"]["@steward/extension-api"],
        "https://plugins.example/steward-extension-api-0.5.0.tgz"
    );
    let nsh: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report.directory.join("nsh.json")).unwrap()).unwrap();
    assert_eq!(nsh["capabilities"]["host"][0], "steward.feedback.toast");
    let test = std::fs::read_to_string(report.directory.join("App.test.tsx")).unwrap();
    assert!(test.contains("app.press"));
    assert!(test.contains("app.host.calls"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn scaffold_rejects_workspace_and_raw_source_sdk_dependencies() {
    for dependency in ["workspace:*", "../packages/extension-api/src/host.ts"] {
        let root = temp_root("scaffold-sdk");
        let target = root.join("com.example.scaffold");
        let error = scaffold_plugin(ScaffoldOptions {
            id: "com.example.scaffold".into(),
            name: "Scaffold".into(),
            version: "0.1.0".into(),
            command: None,
            sdk_dependency: Some(dependency.into()),
            directory: target.clone(),
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("must name a released package"));
        assert!(!target.exists());
        std::fs::remove_dir_all(root).ok();
    }
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
fn developer_replacement_accepts_the_same_build_without_weakening_verification() {
    let root = temp_root("developer-replace");
    let signer = Signer::new();
    let first = signer.zip(&root, "1.0.0", 1);
    let ordinary = store(&root, &signer);
    ordinary.install(&first).unwrap();
    assert!(ordinary.replace_development(&first).is_err());
    assert!(ordinary.install(&first).is_err());

    let developer = PluginStore::open(
        PluginStoreConfig::new(
            root.join("data"),
            root.join("data").join("plugins"),
            vec![signer.root_base64.clone()],
        )
        .with_developer_mode(true),
    )
    .unwrap();
    let replaced = developer.replace_development(&first).unwrap();
    assert_eq!(replaced.version, "1.0.0");
    assert_eq!(replaced.previous_version.as_deref(), Some("1.0.0"));
    let doctor = developer.doctor("com.example.test");
    assert!(doctor.passed, "{doctor:#?}");
    let bad_layout = root.join("bad-layout");
    std::fs::create_dir_all(&bad_layout).unwrap();
    std::fs::copy(
        root.join("data/plugins/com.example.test/plugin.json"),
        bad_layout.join("plugin.json"),
    )
    .unwrap();
    std::fs::copy(
        root.join("data/plugins/com.example.test/plugin.nshpkg"),
        bad_layout.join("plugin.nshpkg"),
    )
    .unwrap();
    std::fs::write(bad_layout.join("README.txt"), "unexpected").unwrap();
    let bad = developer.doctor(&bad_layout.to_string_lossy());
    assert!(!bad.passed);
    assert_eq!(bad.stage, Some(steward_plugin_store::DoctorStage::Layout));
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
    assert_eq!(index.entries[0].author.as_deref(), Some("Test Author"));
    assert_eq!(index.entries[0].command_count, 1);
    assert!(index.entries[0]
        .icon
        .as_deref()
        .is_some_and(|icon| icon.starts_with("<svg")));
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
    store
        .register_publisher(
            PublisherRegistrationRequest::new(
                "pub.example",
                "Example Publisher",
                "plugins@example.com",
                vec!["example.com".into()],
            ),
            &accepted,
            "release-operator",
        )
        .unwrap();
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

#[test]
fn pdx4_publisher_rotation_and_recovery_are_isolated_and_audited() {
    let root = temp_root("pdx4-publishers");
    let alpha = Signer::new();
    let beta = Signer::with_identity("dev.beta.tools", "dev.beta.*");
    let store = store_with_roots(
        &root,
        &[alpha.root_base64.clone(), beta.root_base64.clone()],
    );
    let alpha_v1 = alpha.zip_with_capabilities(
        &root.join("alpha"),
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    let beta_v1 = beta.zip_with_capabilities(
        &root.join("beta"),
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    register_signer(
        &store,
        &alpha,
        "alpha.publisher",
        "alpha.example",
        &alpha_v1,
    );
    register_signer(&store, &beta, "beta.publisher", "beta.example", &beta_v1);

    let beta_key = beta.release.public().id();
    let (alpha_v2, alpha_key_2) = alpha.rotated_zip(
        &root.join("alpha-v2"),
        "1.1.0",
        2,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    let alpha_record = store
        .rotate_publisher_key(
            "alpha.publisher",
            &alpha_v2,
            "key-operator",
            "scheduled release-key rotation",
            false,
        )
        .unwrap();
    assert_eq!(alpha_record.status, PublisherStatus::Active);
    assert_eq!(alpha_record.keys[0].status, PublisherKeyStatus::Retired);
    assert_eq!(
        alpha_record.active_key_id(),
        Some(alpha_key_2.public().id().as_str())
    );
    let beta_record = store.publisher("beta.publisher").unwrap();
    assert_eq!(beta_record.active_key_id(), Some(beta_key.as_str()));
    assert_eq!(beta_record.keys.len(), 1);

    let (alpha_v3, alpha_key_3) = alpha.rotated_zip(
        &root.join("alpha-v3"),
        "1.2.0",
        3,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    let recovered = store
        .rotate_publisher_key(
            "alpha.publisher",
            &alpha_v3,
            "security-operator",
            "previous release key was compromised",
            true,
        )
        .unwrap();
    assert_eq!(
        recovered.active_key_id(),
        Some(alpha_key_3.public().id().as_str())
    );
    assert_eq!(
        recovered
            .keys
            .iter()
            .filter(|key| key.status == PublisherKeyStatus::Revoked)
            .count(),
        2
    );
    assert_eq!(
        store.publisher("beta.publisher").unwrap().active_key_id(),
        Some(beta_key.as_str())
    );

    let audit = store.operator_audit().unwrap();
    assert_eq!(audit.len(), 4);
    assert_eq!(audit[0].sequence, 1);
    assert_eq!(audit[3].action, "publisher_key_recovered");
    assert!(audit
        .windows(2)
        .all(|window| window[1].previous_hash == window[0].hash));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pdx4_public_marketplace_requires_registration_and_reports_capability_delta() {
    let root = temp_root("pdx4-public-marketplace");
    let publisher = Signer::new();
    let unregistered = Signer::with_identity("dev.gamma.tools", "dev.gamma.*");
    let store = store_with_roots(
        &root,
        &[
            publisher.root_base64.clone(),
            unregistered.root_base64.clone(),
        ],
    );
    let v1 = publisher.zip_with_capabilities(
        &root.join("publisher-v1"),
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    register_signer(
        &store,
        &publisher,
        "registered.publisher",
        "publisher.example",
        &v1,
    );
    let v2 = publisher.zip_with_capabilities(
        &root.join("publisher-v2"),
        "1.1.0",
        2,
        serde_json::json!({
            "host": ["steward.storage.get", "steward.clipboard.write"]
        }),
    );
    let gamma = unregistered.zip_with_capabilities(
        &root.join("unregistered"),
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    for (index, package) in [&v1, &v2, &gamma].into_iter().enumerate() {
        let file_name = package.file_name().unwrap().to_string_lossy();
        let destination = store
            .marketplace_dir()
            .join("packages")
            .join(format!("pdx4-{index}-{file_name}"));
        std::fs::copy(package, destination).unwrap();
    }

    let report = store.sync_public_marketplace().unwrap();
    assert_eq!(report.entries, 2);
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("not registered and active")));
    let index = store.marketplace().unwrap();
    let v2_entry = index
        .entries
        .iter()
        .find(|entry| entry.version == "1.1.0")
        .unwrap();
    assert_eq!(
        v2_entry.publisher_id.as_deref(),
        Some("registered.publisher")
    );
    let admission = v2_entry.admission.as_ref().unwrap();
    assert_eq!(admission.previous_version.as_deref(), Some("1.0.0"));
    assert!(admission
        .capability_delta
        .added
        .iter()
        .any(|capability| capability.permission == "steward.clipboard.write"));

    let quarantine = store
        .create_quarantine(QuarantineRequest::new(
            QuarantineTarget::Package {
                package_sha256: v2_entry.package_sha256.clone(),
            },
            "capability regression discovered during release review",
            "release-operator",
            Some("INC-PDX4-001".into()),
        ))
        .unwrap();
    assert_eq!(quarantine.status, QuarantineStatus::Active);
    let quarantined_sync = store.sync_public_marketplace().unwrap();
    assert_eq!(quarantined_sync.entries, 1);
    assert!(quarantined_sync
        .errors
        .iter()
        .any(|error| error.contains("quarantined")));
    assert_eq!(store.marketplace().unwrap().entries[0].version, "1.0.0");

    store
        .release_quarantine(
            &quarantine.id,
            "release-operator",
            "false positive confirmed by package owner",
        )
        .unwrap();
    let restored = store.sync_public_marketplace().unwrap();
    assert_eq!(restored.entries, 2);
    assert!(store.operator_audit().unwrap().iter().any(|record| {
        record.action == "marketplace_quarantine_released" && record.subject == quarantine.id
    }));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pdx4_three_independent_publishers_exercise_unrelated_capability_domains() {
    let root = temp_root("pdx4-independent-publishers");
    let filesystem = Signer::with_identity("com.alpha.notes", "com.alpha.*");
    let network = Signer::with_identity("io.beta.feeds", "io.beta.*");
    let process = Signer::with_identity("dev.gamma.tools", "dev.gamma.*");
    let store = store_with_roots(
        &root,
        &[
            filesystem.root_base64.clone(),
            network.root_base64.clone(),
            process.root_base64.clone(),
        ],
    );
    let filesystem_package = filesystem.zip_with_capabilities(
        &root.join("filesystem"),
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.fs.pick", "steward.fs.walk", "steward.preferences.all"],
            "fs": { "read": ["${dataDir}"] }
        }),
    );
    let network_package = network.zip_with_capabilities(
        &root.join("network"),
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.net.fetch"],
            "network": {
                "http": [{
                    "host": "feeds.beta.example",
                    "schemes": ["https"],
                    "methods": ["GET"],
                    "pathPrefixes": ["/v1"]
                }]
            }
        }),
    );
    let process_package = process.zip_with_capabilities(
        &root.join("process"),
        "1.0.0",
        1,
        serde_json::json!({
            "host": ["steward.exec.run", "steward.exec.cancel"],
            "process": { "execute": ["git.exe"] }
        }),
    );
    register_signer(
        &store,
        &filesystem,
        "alpha.files",
        "alpha.example",
        &filesystem_package,
    );
    register_signer(
        &store,
        &network,
        "beta.feeds",
        "beta.example",
        &network_package,
    );
    register_signer(
        &store,
        &process,
        "gamma.tools",
        "gamma.example",
        &process_package,
    );
    for (index, package) in [&filesystem_package, &network_package, &process_package]
        .into_iter()
        .enumerate()
    {
        let file_name = package.file_name().unwrap().to_string_lossy();
        std::fs::copy(
            package,
            store
                .marketplace_dir()
                .join("packages")
                .join(format!("publisher-{index}-{file_name}")),
        )
        .unwrap();
    }

    let report = store.sync_public_marketplace().unwrap();
    assert_eq!(report.entries, 3, "{:?}", report.errors);
    let publishers = store
        .marketplace()
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.publisher_id.unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(publishers.len(), 3);
    assert!(store
        .marketplace()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.id == "com.alpha.notes"));
    assert!(store
        .marketplace()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.id == "io.beta.feeds"));
    assert!(store
        .marketplace()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.id == "dev.gamma.tools"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pdx4_quarantine_can_publish_a_root_signed_revocation_response() {
    let root = temp_root("pdx4-revocation-response");
    let publisher = Signer::new();
    let store = store(&root, &publisher);
    let package = publisher.zip_with_capabilities(
        &root.join("package"),
        "1.0.0",
        1,
        serde_json::json!({ "host": ["steward.storage.get"] }),
    );
    register_signer(
        &store,
        &publisher,
        "incident.publisher",
        "incident.example",
        &package,
    );
    let destination = store
        .marketplace_dir()
        .join("packages")
        .join(package.file_name().unwrap());
    std::fs::copy(&package, destination).unwrap();
    store.sync_public_marketplace().unwrap();
    let package_sha256 = store.marketplace().unwrap().entries[0]
        .package_sha256
        .clone();
    let quarantine = store
        .create_quarantine(QuarantineRequest::new(
            QuarantineTarget::Package {
                package_sha256: package_sha256.clone(),
            },
            "malicious package reported by multiple users",
            "incident-commander",
            Some("INC-PDX4-002".into()),
        ))
        .unwrap();
    let root_key = root.join("offline-root.secret");
    std::fs::write(&root_key, publisher.root.to_base64()).unwrap();
    let revocation = root.join("revocations-pdx4.json");
    let (record, created) = store
        .revoke_quarantine(
            &quarantine.id,
            &root_key,
            None,
            1,
            &revocation,
            "security-operator",
        )
        .unwrap();
    assert_eq!(created.packages, vec![package_sha256.clone()]);
    assert_eq!(
        record
            .revocation
            .as_ref()
            .map(|revocation| revocation.sequence),
        Some(1)
    );

    store.apply_revocations(&revocation).unwrap();
    let inspection = store.inspect(&package).unwrap();
    assert!(!inspection.trusted);
    assert!(inspection
        .trust_error
        .as_deref()
        .is_some_and(|error| error.contains("revoked")));
    assert!(store.operator_audit().unwrap().iter().any(|record| {
        record.action == "quarantine_revocations_published" && record.subject == quarantine.id
    }));
    std::fs::remove_dir_all(root).ok();
}
