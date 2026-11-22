use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::revocation::{create_revocation_list, CreatedRevocationList};
use crate::{atomic_write, safe_relative_path, unix_seconds, PackageInspection, PluginStore};

pub const OPERATIONS_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublisherStatus {
    Active,
    Suspended,
    Revoked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublisherKeyStatus {
    Active,
    Retired,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublisherRegistrationRequest {
    pub id: String,
    pub display_name: String,
    pub contact: String,
    pub domains: Vec<String>,
}

impl PublisherRegistrationRequest {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        contact: impl Into<String>,
        domains: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            contact: contact.into(),
            domains,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublisherKeyRecord {
    pub key_id: String,
    pub status: PublisherKeyStatus,
    pub registered_at: i64,
    pub changed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublisherRecord {
    pub id: String,
    pub display_name: String,
    pub contact: String,
    pub domains: Vec<String>,
    pub status: PublisherStatus,
    pub keys: Vec<PublisherKeyRecord>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl PublisherRecord {
    pub fn active_key_id(&self) -> Option<&str> {
        self.keys
            .iter()
            .find(|key| key.status == PublisherKeyStatus::Active)
            .map(|key| key.key_id.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperatorAuditRecord {
    pub sequence: u64,
    pub recorded_at: i64,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: String,
    pub previous_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QuarantineTarget {
    Package {
        package_sha256: String,
    },
    Plugin {
        plugin_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    ReleaseKey {
        key_id: String,
    },
    Publisher {
        publisher_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineRequest {
    pub target: QuarantineTarget,
    pub reason: String,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_ref: Option<String>,
}

impl QuarantineRequest {
    pub fn new(
        target: QuarantineTarget,
        reason: impl Into<String>,
        operator: impl Into<String>,
        incident_ref: Option<String>,
    ) -> Self {
        Self {
            target,
            reason: reason.into(),
            operator: operator.into(),
            incident_ref,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineStatus {
    Active,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineRevocation {
    pub sequence: u64,
    pub path: PathBuf,
    pub keys: Vec<String>,
    pub packages: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineRecord {
    pub id: String,
    pub target: QuarantineTarget,
    pub reason: String,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_ref: Option<String>,
    pub status: QuarantineStatus,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation: Option<QuarantineRevocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationsState {
    schema_version: u16,
    publishers: BTreeMap<String, PublisherRecord>,
    quarantines: Vec<QuarantineRecord>,
    audit: Vec<OperatorAuditRecord>,
}

impl Default for OperationsState {
    fn default() -> Self {
        Self {
            schema_version: OPERATIONS_SCHEMA_VERSION,
            publishers: BTreeMap::new(),
            quarantines: Vec::new(),
            audit: Vec::new(),
        }
    }
}

pub(crate) fn register_publisher(
    store: &PluginStore,
    request: PublisherRegistrationRequest,
    signed_package: &Path,
    actor: &str,
) -> Result<PublisherRecord> {
    let inspection = inspect_verified_package(store, signed_package)?;
    validate_publisher_request(&request)?;
    validate_actor(actor)?;
    let key_id = inspection
        .publisher_key_id
        .clone()
        .context("verified package has no publisher key identity")?;
    let mut state = load(store)?;
    if state.publishers.contains_key(&request.id) {
        anyhow::bail!("publisher '{}' is already registered", request.id);
    }
    if let Some(existing) = publisher_for_key(&state, &key_id) {
        anyhow::bail!(
            "release key '{key_id}' is already registered to publisher '{}'",
            existing.id
        );
    }
    let now = unix_seconds();
    let publisher = PublisherRecord {
        id: request.id.clone(),
        display_name: request.display_name.trim().to_owned(),
        contact: request.contact.trim().to_owned(),
        domains: normalize_domains(request.domains)?,
        status: PublisherStatus::Active,
        keys: vec![PublisherKeyRecord {
            key_id: key_id.clone(),
            status: PublisherKeyStatus::Active,
            registered_at: now,
            changed_at: now,
            reason: None,
        }],
        created_at: now,
        updated_at: now,
    };
    state
        .publishers
        .insert(request.id.clone(), publisher.clone());
    append_audit(
        &mut state,
        actor,
        "publisher_registered",
        &request.id,
        &format!("activated release key {key_id}"),
    );
    save(store, &state)?;
    Ok(publisher)
}

pub(crate) fn rotate_publisher_key(
    store: &PluginStore,
    publisher_id: &str,
    signed_package: &Path,
    actor: &str,
    reason: &str,
    recovery: bool,
) -> Result<PublisherRecord> {
    let inspection = inspect_verified_package(store, signed_package)?;
    validate_actor(actor)?;
    validate_reason("key change reason", reason)?;
    let replacement_key = inspection
        .publisher_key_id
        .clone()
        .context("verified package has no publisher key identity")?;
    let mut state = load(store)?;
    let publisher = state
        .publishers
        .get(publisher_id)
        .cloned()
        .with_context(|| format!("publisher '{publisher_id}' is not registered"))?;
    if publisher.status == PublisherStatus::Revoked {
        anyhow::bail!("publisher '{publisher_id}' is permanently revoked");
    }
    if publisher
        .keys
        .iter()
        .any(|key| key.key_id == replacement_key)
    {
        anyhow::bail!(
            "release key '{replacement_key}' is already registered to publisher '{publisher_id}'"
        );
    }
    if let Some(existing) = publisher_for_key(&state, &replacement_key) {
        anyhow::bail!(
            "release key '{replacement_key}' is already registered to publisher '{}'",
            existing.id
        );
    }

    let now = unix_seconds();
    let replacement_status = if recovery {
        PublisherKeyStatus::Revoked
    } else {
        PublisherKeyStatus::Retired
    };
    let publisher = state
        .publishers
        .get_mut(publisher_id)
        .expect("publisher checked above");
    for key in &mut publisher.keys {
        if recovery {
            if key.status != PublisherKeyStatus::Revoked {
                key.status = PublisherKeyStatus::Revoked;
                key.changed_at = now;
                key.reason = Some(reason.trim().to_owned());
            }
        } else if key.status == PublisherKeyStatus::Active {
            key.status = replacement_status;
            key.changed_at = now;
            key.reason = Some(reason.trim().to_owned());
        }
    }
    publisher.keys.push(PublisherKeyRecord {
        key_id: replacement_key.clone(),
        status: PublisherKeyStatus::Active,
        registered_at: now,
        changed_at: now,
        reason: Some(reason.trim().to_owned()),
    });
    publisher.status = PublisherStatus::Active;
    publisher.updated_at = now;
    let publisher = publisher.clone();
    append_audit(
        &mut state,
        actor,
        if recovery {
            "publisher_key_recovered"
        } else {
            "publisher_key_rotated"
        },
        publisher_id,
        &format!("activated release key {replacement_key}: {reason}"),
    );
    save(store, &state)?;
    Ok(publisher)
}

pub(crate) fn revoke_publisher_key(
    store: &PluginStore,
    publisher_id: &str,
    key_id: &str,
    actor: &str,
    reason: &str,
) -> Result<PublisherRecord> {
    validate_actor(actor)?;
    validate_reason("key revocation reason", reason)?;
    let mut state = load(store)?;
    let publisher = state
        .publishers
        .get_mut(publisher_id)
        .with_context(|| format!("publisher '{publisher_id}' is not registered"))?;
    let key = publisher
        .keys
        .iter_mut()
        .find(|key| key.key_id == key_id)
        .with_context(|| {
            format!("release key '{key_id}' is not registered to publisher '{publisher_id}'")
        })?;
    if key.status == PublisherKeyStatus::Revoked {
        anyhow::bail!("release key '{key_id}' is already revoked");
    }
    let was_active = key.status == PublisherKeyStatus::Active;
    key.status = PublisherKeyStatus::Revoked;
    key.changed_at = unix_seconds();
    key.reason = Some(reason.trim().to_owned());
    publisher.updated_at = key.changed_at;
    if was_active {
        publisher.status = PublisherStatus::Suspended;
    }
    let publisher = publisher.clone();
    append_audit(
        &mut state,
        actor,
        "publisher_key_revoked",
        publisher_id,
        &format!("revoked release key {key_id}: {reason}"),
    );
    save(store, &state)?;
    Ok(publisher)
}

pub(crate) fn set_publisher_status(
    store: &PluginStore,
    publisher_id: &str,
    status: PublisherStatus,
    actor: &str,
    reason: &str,
) -> Result<PublisherRecord> {
    validate_actor(actor)?;
    validate_reason("publisher status reason", reason)?;
    let mut state = load(store)?;
    let publisher = state
        .publishers
        .get_mut(publisher_id)
        .with_context(|| format!("publisher '{publisher_id}' is not registered"))?;
    if publisher.status == PublisherStatus::Revoked && status != PublisherStatus::Revoked {
        anyhow::bail!("publisher '{publisher_id}' is permanently revoked");
    }
    if status == PublisherStatus::Active
        && !publisher
            .keys
            .iter()
            .any(|key| key.status == PublisherKeyStatus::Active)
    {
        anyhow::bail!(
            "publisher '{publisher_id}' needs an active release key before it can be reactivated"
        );
    }
    publisher.status = status;
    publisher.updated_at = unix_seconds();
    let publisher = publisher.clone();
    append_audit(
        &mut state,
        actor,
        "publisher_status_changed",
        publisher_id,
        &format!("status is now {status:?}: {reason}"),
    );
    save(store, &state)?;
    Ok(publisher)
}

pub(crate) fn publishers(store: &PluginStore) -> Result<Vec<PublisherRecord>> {
    Ok(load(store)?.publishers.into_values().collect())
}

pub(crate) fn publisher(store: &PluginStore, publisher_id: &str) -> Result<PublisherRecord> {
    load(store)?
        .publishers
        .remove(publisher_id)
        .with_context(|| format!("publisher '{publisher_id}' is not registered"))
}

pub(crate) fn operator_audit(store: &PluginStore) -> Result<Vec<OperatorAuditRecord>> {
    Ok(load(store)?.audit)
}

pub(crate) fn authorized_publisher(
    store: &PluginStore,
    key_id: &str,
) -> Result<Option<PublisherRecord>> {
    let state = load(store)?;
    Ok(publisher_for_key(&state, key_id)
        .filter(|publisher| publisher.status == PublisherStatus::Active)
        .filter(|publisher| {
            publisher
                .keys
                .iter()
                .any(|key| key.key_id == key_id && key.status == PublisherKeyStatus::Active)
        })
        .cloned())
}

pub(crate) fn create_quarantine(
    store: &PluginStore,
    request: QuarantineRequest,
) -> Result<QuarantineRecord> {
    validate_actor(&request.operator)?;
    validate_reason("quarantine reason", &request.reason)?;
    validate_quarantine_target(&request.target)?;
    let mut state = load(store)?;
    if state
        .quarantines
        .iter()
        .any(|record| record.status == QuarantineStatus::Active && record.target == request.target)
    {
        anyhow::bail!("the requested public marketplace target is already quarantined");
    }
    let now = unix_seconds();
    let id = format!(
        "quarantine-{now}-{}",
        state.quarantines.len().saturating_add(1)
    );
    let record = QuarantineRecord {
        id: id.clone(),
        target: request.target.clone(),
        reason: request.reason.trim().to_owned(),
        operator: request.operator.clone(),
        incident_ref: request
            .incident_ref
            .filter(|value| !value.trim().is_empty()),
        status: QuarantineStatus::Active,
        created_at: now,
        released_at: None,
        release_reason: None,
        revocation: None,
    };
    state.quarantines.push(record.clone());
    append_audit(
        &mut state,
        &request.operator,
        "marketplace_quarantined",
        &id,
        &format!("{:?}: {}", request.target, request.reason.trim()),
    );
    save(store, &state)?;
    Ok(record)
}

pub(crate) fn quarantines(store: &PluginStore) -> Result<Vec<QuarantineRecord>> {
    Ok(load(store)?.quarantines)
}

pub(crate) fn release_quarantine(
    store: &PluginStore,
    quarantine_id: &str,
    operator: &str,
    reason: &str,
) -> Result<QuarantineRecord> {
    validate_actor(operator)?;
    validate_reason("quarantine release reason", reason)?;
    let mut state = load(store)?;
    let record = state
        .quarantines
        .iter_mut()
        .find(|record| record.id == quarantine_id)
        .with_context(|| format!("quarantine '{quarantine_id}' does not exist"))?;
    if record.status != QuarantineStatus::Active {
        anyhow::bail!("quarantine '{quarantine_id}' is not active");
    }
    record.status = QuarantineStatus::Released;
    record.released_at = Some(unix_seconds());
    record.release_reason = Some(reason.trim().to_owned());
    let record = record.clone();
    append_audit(
        &mut state,
        operator,
        "marketplace_quarantine_released",
        quarantine_id,
        reason,
    );
    save(store, &state)?;
    Ok(record)
}

pub(crate) fn package_quarantined(
    store: &PluginStore,
    inspection: &PackageInspection,
) -> Result<Option<QuarantineRecord>> {
    let state = load(store)?;
    let publisher = inspection
        .publisher_key_id
        .as_deref()
        .and_then(|key_id| publisher_for_key(&state, key_id));
    Ok(state
        .quarantines
        .iter()
        .filter(|record| record.status == QuarantineStatus::Active)
        .find(|record| match &record.target {
            QuarantineTarget::Package { package_sha256 } => {
                package_sha256.eq_ignore_ascii_case(&inspection.package_sha256)
            }
            QuarantineTarget::Plugin { plugin_id, version } => {
                plugin_id == &inspection.app_id
                    && version
                        .as_deref()
                        .is_none_or(|version| version == inspection.version)
            }
            QuarantineTarget::ReleaseKey { key_id } => {
                inspection.publisher_key_id.as_ref() == Some(key_id)
            }
            QuarantineTarget::Publisher { publisher_id } => publisher
                .as_ref()
                .is_some_and(|publisher| publisher.id == *publisher_id),
        })
        .cloned())
}

pub(crate) fn revoke_quarantine(
    store: &PluginStore,
    quarantine_id: &str,
    root_key_path: &Path,
    previous: Option<&Path>,
    sequence: u64,
    output: &Path,
    operator: &str,
) -> Result<(QuarantineRecord, CreatedRevocationList)> {
    validate_actor(operator)?;
    let mut state = load(store)?;
    let record = state
        .quarantines
        .iter()
        .find(|record| record.id == quarantine_id)
        .cloned()
        .with_context(|| format!("quarantine '{quarantine_id}' does not exist"))?;
    if record.status != QuarantineStatus::Active {
        anyhow::bail!("quarantine '{quarantine_id}' is not active");
    }
    let (keys, packages) = revocation_targets(store, &state, &record.target)?;
    let created =
        create_revocation_list(root_key_path, previous, sequence, &keys, &packages, output)?;
    let stored = state
        .quarantines
        .iter_mut()
        .find(|record| record.id == quarantine_id)
        .expect("quarantine checked above");
    stored.revocation = Some(QuarantineRevocation {
        sequence: created.sequence,
        path: created.path.clone(),
        keys: created.keys.clone(),
        packages: created.packages.clone(),
        created_at: unix_seconds(),
    });
    let stored = stored.clone();
    append_audit(
        &mut state,
        operator,
        "quarantine_revocations_published",
        quarantine_id,
        &format!(
            "root-signed revocation sequence {} written to {}",
            created.sequence,
            created.path.display()
        ),
    );
    save(store, &state)?;
    Ok((stored, created))
}

fn inspect_verified_package(store: &PluginStore, path: &Path) -> Result<PackageInspection> {
    let inspection = store.inspect(path)?;
    if !inspection.installable() {
        anyhow::bail!("publisher operation requires a trusted package accepted by Steward policy");
    }
    Ok(inspection)
}

fn validate_publisher_request(request: &PublisherRegistrationRequest) -> Result<()> {
    let id = request.id.trim();
    if id.is_empty()
        || id.len() > 128
        || !safe_relative_path(Path::new(id))
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        anyhow::bail!("publisher id must contain only ASCII letters, digits, '.', '_', or '-'");
    }
    if request.display_name.trim().is_empty() || request.display_name.len() > 200 {
        anyhow::bail!("publisher display name must be between 1 and 200 bytes");
    }
    if request.contact.trim().is_empty() || request.contact.len() > 320 {
        anyhow::bail!("publisher contact must be between 1 and 320 bytes");
    }
    if request.domains.is_empty() {
        anyhow::bail!("at least one publisher domain is required");
    }
    Ok(())
}

fn normalize_domains(domains: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for domain in domains {
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        if domain.is_empty()
            || domain.len() > 253
            || domain.contains(['/', '\\', ':', '@'])
            || domain.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || !label
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
        {
            anyhow::bail!("invalid publisher domain '{domain}'");
        }
        normalized.insert(domain);
    }
    Ok(normalized.into_iter().collect())
}

fn validate_actor(actor: &str) -> Result<()> {
    if actor.trim().is_empty() || actor.len() > 200 {
        anyhow::bail!("operator identity must be between 1 and 200 bytes");
    }
    Ok(())
}

fn validate_reason(label: &str, reason: &str) -> Result<()> {
    if reason.trim().is_empty() || reason.len() > 2_000 {
        anyhow::bail!("{label} must be between 1 and 2000 bytes");
    }
    Ok(())
}

fn validate_quarantine_target(target: &QuarantineTarget) -> Result<()> {
    match target {
        QuarantineTarget::Package { package_sha256 } => {
            if package_sha256.len() != 64
                || !package_sha256
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                anyhow::bail!("package quarantine target requires a full SHA-256 digest");
            }
        }
        QuarantineTarget::Plugin { plugin_id, version } => {
            if plugin_id.trim().is_empty()
                || !safe_relative_path(Path::new(plugin_id))
                || matches!(plugin_id.as_str(), "." | "..")
            {
                anyhow::bail!("plugin quarantine target requires a plugin id");
            }
            if version
                .as_deref()
                .is_some_and(|version| version.trim().is_empty())
            {
                anyhow::bail!("plugin quarantine version cannot be empty");
            }
            if let Some(version) = version {
                Version::parse(version)
                    .with_context(|| format!("invalid quarantine plugin version '{version}'"))?;
            }
        }
        QuarantineTarget::ReleaseKey { key_id } => {
            if key_id.trim().is_empty() {
                anyhow::bail!("release-key quarantine target requires a key id");
            }
        }
        QuarantineTarget::Publisher { publisher_id } => {
            if publisher_id.trim().is_empty() {
                anyhow::bail!("publisher quarantine target requires a publisher id");
            }
        }
    }
    Ok(())
}

fn publisher_for_key<'a>(state: &'a OperationsState, key_id: &str) -> Option<&'a PublisherRecord> {
    state
        .publishers
        .values()
        .find(|publisher| publisher.keys.iter().any(|key| key.key_id == key_id))
}

fn revocation_targets(
    store: &PluginStore,
    state: &OperationsState,
    target: &QuarantineTarget,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut keys = Vec::new();
    let mut packages = Vec::new();
    match target {
        QuarantineTarget::Package { package_sha256 } => {
            packages.push(package_sha256.to_ascii_lowercase());
        }
        QuarantineTarget::Plugin { plugin_id, version } => {
            let index = store.marketplace()?;
            packages.extend(
                index
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.id == *plugin_id
                            && version
                                .as_deref()
                                .is_none_or(|version| entry.version == version)
                    })
                    .map(|entry| entry.package_sha256.clone()),
            );
            if packages.is_empty() {
                anyhow::bail!(
                    "no marketplace package matches quarantined plugin '{}'",
                    plugin_id
                );
            }
        }
        QuarantineTarget::ReleaseKey { key_id } => keys.push(key_id.clone()),
        QuarantineTarget::Publisher { publisher_id } => {
            let publisher = state
                .publishers
                .get(publisher_id)
                .with_context(|| format!("publisher '{publisher_id}' is not registered"))?;
            keys.extend(publisher.keys.iter().map(|key| key.key_id.clone()));
        }
    }
    keys.sort();
    keys.dedup();
    packages.sort();
    packages.dedup();
    Ok((keys, packages))
}

fn append_audit(
    state: &mut OperationsState,
    actor: &str,
    action: &str,
    subject: &str,
    detail: &str,
) {
    let sequence = state.audit.last().map_or(1, |record| record.sequence + 1);
    let previous_hash = state
        .audit
        .last()
        .map(|record| record.hash.clone())
        .unwrap_or_default();
    let mut record = OperatorAuditRecord {
        sequence,
        recorded_at: unix_seconds(),
        actor: actor.trim().to_owned(),
        action: action.to_owned(),
        subject: subject.to_owned(),
        detail: detail.to_owned(),
        previous_hash,
        hash: String::new(),
    };
    record.hash = audit_hash(&record);
    state.audit.push(record);
}

fn audit_hash(record: &OperatorAuditRecord) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AuditPayload<'a> {
        sequence: u64,
        recorded_at: i64,
        actor: &'a str,
        action: &'a str,
        subject: &'a str,
        detail: &'a str,
        previous_hash: &'a str,
    }
    let payload = serde_json::to_vec(&AuditPayload {
        sequence: record.sequence,
        recorded_at: record.recorded_at,
        actor: &record.actor,
        action: &record.action,
        subject: &record.subject,
        detail: &record.detail,
        previous_hash: &record.previous_hash,
    })
    .expect("operator audit payload is serializable");
    format!("{:x}", Sha256::digest(payload))
}

fn load(store: &PluginStore) -> Result<OperationsState> {
    let path = operations_path(store);
    if !path.exists() {
        return Ok(OperationsState::default());
    }
    let state: OperationsState = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if state.schema_version != OPERATIONS_SCHEMA_VERSION {
        anyhow::bail!(
            "plugin operations schema {} is unsupported; expected {}",
            state.schema_version,
            OPERATIONS_SCHEMA_VERSION
        );
    }
    validate_state(&state)?;
    let mut previous_hash = "";
    for (index, record) in state.audit.iter().enumerate() {
        if record.sequence != (index as u64).saturating_add(1)
            || record.previous_hash != previous_hash
            || record.hash != audit_hash(record)
        {
            anyhow::bail!(
                "operator audit record {} failed integrity validation",
                index + 1
            );
        }
        previous_hash = &record.hash;
    }
    Ok(state)
}

fn validate_state(state: &OperationsState) -> Result<()> {
    for (publisher_id, publisher) in &state.publishers {
        if publisher.id != *publisher_id
            || publisher.display_name.trim().is_empty()
            || publisher.contact.trim().is_empty()
            || publisher.domains.is_empty()
        {
            anyhow::bail!("publisher '{publisher_id}' has invalid identity metadata");
        }
        let mut keys = BTreeSet::new();
        let mut active_keys = 0;
        for key in &publisher.keys {
            if key.key_id.trim().is_empty() || !keys.insert(key.key_id.as_str()) {
                anyhow::bail!("publisher '{publisher_id}' has an invalid or duplicate release key");
            }
            if key.status == PublisherKeyStatus::Active {
                active_keys += 1;
            }
        }
        if active_keys > 1 {
            anyhow::bail!("publisher '{publisher_id}' has more than one active release key");
        }
        if publisher.status == PublisherStatus::Active && active_keys != 1 {
            anyhow::bail!(
                "active publisher '{publisher_id}' must have exactly one active release key"
            );
        }
    }
    let mut quarantines = BTreeSet::new();
    for record in &state.quarantines {
        if record.id.trim().is_empty()
            || !quarantines.insert(record.id.as_str())
            || record.reason.trim().is_empty()
            || record.operator.trim().is_empty()
        {
            anyhow::bail!("plugin operations state contains an invalid quarantine record");
        }
    }
    Ok(())
}

fn save(store: &PluginStore, state: &OperationsState) -> Result<()> {
    atomic_write(
        &operations_path(store),
        &serde_json::to_vec_pretty(state).context("serialize plugin operations state")?,
    )
}

fn operations_path(store: &PluginStore) -> PathBuf {
    store.data_dir().join("plugin-operations.json")
}
