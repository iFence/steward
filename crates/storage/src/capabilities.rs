use anyhow::{bail, Context as _, Result};
use rusqlite::{params, Connection, OptionalExtension};
use semver::{Version, VersionReq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCapabilityGrant {
    pub plugin_id: String,
    pub publisher_fingerprint: String,
    pub version_range: String,
    pub capability: String,
    pub scope: String,
    pub granted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub id: i64,
    pub plugin_id: String,
    pub publisher_fingerprint: String,
    pub version_range: String,
    pub capability: String,
    pub scope: String,
    pub granted_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCapabilityAuditEntry {
    pub plugin_id: String,
    pub publisher_fingerprint: String,
    pub capability: String,
    pub scope: String,
    pub outcome: String,
    pub occurred_at: i64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAuditEntry {
    pub id: i64,
    pub plugin_id: String,
    pub publisher_fingerprint: String,
    pub capability: String,
    pub scope: String,
    pub outcome: String,
    pub occurred_at: i64,
    pub detail: Option<String>,
}

pub(crate) fn grant(conn: &Connection, request: &NewCapabilityGrant) -> Result<CapabilityGrant> {
    validate_grant(request)?;
    let tx = conn
        .unchecked_transaction()
        .context("begin capability grant")?;
    if let Some(existing) = tx
        .query_row(
            "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                    granted_at, revoked_at
             FROM capability_grants
             WHERE plugin_id = ?1 AND publisher_fingerprint = ?2 AND version_range = ?3
               AND capability = ?4 AND scope = ?5 AND revoked_at IS NULL",
            params![
                request.plugin_id,
                request.publisher_fingerprint,
                request.version_range,
                request.capability,
                request.scope
            ],
            row_to_grant,
        )
        .optional()
        .context("find active capability grant")?
    {
        tx.commit().context("commit capability grant lookup")?;
        return Ok(existing);
    }

    tx.execute(
        "INSERT INTO capability_grants (
            plugin_id, publisher_fingerprint, version_range, capability, scope,
            granted_at, revoked_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        params![
            request.plugin_id,
            request.publisher_fingerprint,
            request.version_range,
            request.capability,
            request.scope,
            request.granted_at
        ],
    )
    .context("insert capability grant")?;
    let id = tx.last_insert_rowid();
    insert_audit(
        &tx,
        &NewCapabilityAuditEntry {
            plugin_id: request.plugin_id.clone(),
            publisher_fingerprint: request.publisher_fingerprint.clone(),
            capability: request.capability.clone(),
            scope: request.scope.clone(),
            outcome: "granted".into(),
            occurred_at: request.granted_at,
            detail: Some(format!("version_range={}", request.version_range)),
        },
    )?;
    let grant = tx
        .query_row(
            "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                    granted_at, revoked_at
             FROM capability_grants WHERE id = ?1",
            [id],
            row_to_grant,
        )
        .context("read inserted capability grant")?;
    tx.commit().context("commit capability grant")?;
    Ok(grant)
}

pub(crate) fn revoke(conn: &Connection, id: i64, revoked_at: i64) -> Result<CapabilityGrant> {
    let tx = conn
        .unchecked_transaction()
        .context("begin capability revocation")?;
    let grant = tx
        .query_row(
            "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                    granted_at, revoked_at
             FROM capability_grants WHERE id = ?1",
            [id],
            row_to_grant,
        )
        .optional()
        .context("find capability grant")?
        .with_context(|| format!("capability grant {id} does not exist"))?;
    if grant.revoked_at.is_some() {
        bail!("capability grant {id} is already revoked");
    }
    tx.execute(
        "UPDATE capability_grants SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
        params![revoked_at, id],
    )
    .context("revoke capability grant")?;
    insert_audit(
        &tx,
        &NewCapabilityAuditEntry {
            plugin_id: grant.plugin_id.clone(),
            publisher_fingerprint: grant.publisher_fingerprint.clone(),
            capability: grant.capability.clone(),
            scope: grant.scope.clone(),
            outcome: "revoked".into(),
            occurred_at: revoked_at,
            detail: Some(format!("grant_id={id}")),
        },
    )?;
    let revoked = tx
        .query_row(
            "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                    granted_at, revoked_at
             FROM capability_grants WHERE id = ?1",
            [id],
            row_to_grant,
        )
        .context("read revoked capability grant")?;
    tx.commit().context("commit capability revocation")?;
    Ok(revoked)
}

pub(crate) fn active_for_plugin(
    conn: &Connection,
    plugin_id: &str,
) -> Result<Vec<CapabilityGrant>> {
    query_grants(
        conn,
        "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                granted_at, revoked_at
         FROM capability_grants
         WHERE plugin_id = ?1 AND revoked_at IS NULL
         ORDER BY capability, scope, granted_at",
        plugin_id,
    )
}

pub(crate) fn history_for_plugin(
    conn: &Connection,
    plugin_id: &str,
) -> Result<Vec<CapabilityGrant>> {
    query_grants(
        conn,
        "SELECT id, plugin_id, publisher_fingerprint, version_range, capability, scope,
                granted_at, revoked_at
         FROM capability_grants
         WHERE plugin_id = ?1
         ORDER BY granted_at, id",
        plugin_id,
    )
}

pub(crate) fn matching_active(
    conn: &Connection,
    plugin_id: &str,
    publisher_fingerprint: &str,
    version: &str,
) -> Result<Vec<CapabilityGrant>> {
    let version = Version::parse(version).context("parse plugin version")?;
    Ok(active_for_plugin(conn, plugin_id)?
        .into_iter()
        .filter(|grant| grant.publisher_fingerprint == publisher_fingerprint)
        .filter(|grant| {
            VersionReq::parse(&grant.version_range)
                .map(|requirement| requirement.matches(&version))
                .unwrap_or(false)
        })
        .collect())
}

pub(crate) fn append_audit(
    conn: &Connection,
    entry: &NewCapabilityAuditEntry,
) -> Result<CapabilityAuditEntry> {
    validate_audit(entry)?;
    insert_audit(conn, entry)?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, plugin_id, publisher_fingerprint, capability, scope, outcome,
                occurred_at, detail
         FROM capability_audit WHERE id = ?1",
        [id],
        |row| {
            Ok(CapabilityAuditEntry {
                id: row.get(0)?,
                plugin_id: row.get(1)?,
                publisher_fingerprint: row.get(2)?,
                capability: row.get(3)?,
                scope: row.get(4)?,
                outcome: row.get(5)?,
                occurred_at: row.get(6)?,
                detail: row.get(7)?,
            })
        },
    )
    .context("read capability audit entry")
}

pub(crate) fn audit_for_plugin(
    conn: &Connection,
    plugin_id: &str,
) -> Result<Vec<CapabilityAuditEntry>> {
    let mut statement = conn
        .prepare(
            "SELECT id, plugin_id, publisher_fingerprint, capability, scope, outcome,
                    occurred_at, detail
             FROM capability_audit
             WHERE plugin_id = ?1
             ORDER BY occurred_at, id",
        )
        .context("prepare capability audit query")?;
    let rows = statement
        .query_map([plugin_id], |row| {
            Ok(CapabilityAuditEntry {
                id: row.get(0)?,
                plugin_id: row.get(1)?,
                publisher_fingerprint: row.get(2)?,
                capability: row.get(3)?,
                scope: row.get(4)?,
                outcome: row.get(5)?,
                occurred_at: row.get(6)?,
                detail: row.get(7)?,
            })
        })
        .context("query capability audit entries")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect capability audit entries")
}

fn query_grants(conn: &Connection, sql: &str, plugin_id: &str) -> Result<Vec<CapabilityGrant>> {
    let mut statement = conn
        .prepare(sql)
        .context("prepare capability grant query")?;
    let rows = statement
        .query_map([plugin_id], row_to_grant)
        .context("query capability grants")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect capability grants")
}

fn row_to_grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<CapabilityGrant> {
    Ok(CapabilityGrant {
        id: row.get(0)?,
        plugin_id: row.get(1)?,
        publisher_fingerprint: row.get(2)?,
        version_range: row.get(3)?,
        capability: row.get(4)?,
        scope: row.get(5)?,
        granted_at: row.get(6)?,
        revoked_at: row.get(7)?,
    })
}

fn insert_audit(conn: &Connection, entry: &NewCapabilityAuditEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO capability_audit (
            plugin_id, publisher_fingerprint, capability, scope, outcome, occurred_at, detail
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            entry.plugin_id,
            entry.publisher_fingerprint,
            entry.capability,
            entry.scope,
            entry.outcome,
            entry.occurred_at,
            entry.detail
        ],
    )
    .context("insert capability audit entry")?;
    Ok(())
}

fn validate_grant(grant: &NewCapabilityGrant) -> Result<()> {
    for (name, value) in [
        ("plugin_id", grant.plugin_id.as_str()),
        (
            "publisher_fingerprint",
            grant.publisher_fingerprint.as_str(),
        ),
        ("version_range", grant.version_range.as_str()),
        ("capability", grant.capability.as_str()),
        ("scope", grant.scope.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("capability grant {name} must not be empty");
        }
    }
    VersionReq::parse(&grant.version_range).context("validate capability version range")?;
    Ok(())
}

fn validate_audit(entry: &NewCapabilityAuditEntry) -> Result<()> {
    for (name, value) in [
        ("plugin_id", entry.plugin_id.as_str()),
        (
            "publisher_fingerprint",
            entry.publisher_fingerprint.as_str(),
        ),
        ("capability", entry.capability.as_str()),
        ("scope", entry.scope.as_str()),
        ("outcome", entry.outcome.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("capability audit {name} must not be empty");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Storage;

    fn grant_request(version_range: &str) -> NewCapabilityGrant {
        NewCapabilityGrant {
            plugin_id: "com.example.test".into(),
            publisher_fingerprint: "SHA256:abc".into(),
            version_range: version_range.into(),
            capability: "steward.network.http".into(),
            scope: "domain=example.com".into(),
            granted_at: 100,
        }
    }

    #[test]
    fn grants_are_scoped_to_publisher_and_version() {
        let storage = Storage::open_in_memory().unwrap();
        grant(&storage.conn, &grant_request("^1.2")).unwrap();

        let matching =
            matching_active(&storage.conn, "com.example.test", "SHA256:abc", "1.3.0").unwrap();
        assert_eq!(matching.len(), 1);
        assert!(
            matching_active(&storage.conn, "com.example.test", "SHA256:other", "1.3.0")
                .unwrap()
                .is_empty()
        );
        assert!(
            matching_active(&storage.conn, "com.example.test", "SHA256:abc", "2.0.0")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn revocation_keeps_history_and_writes_an_audit_record() {
        let storage = Storage::open_in_memory().unwrap();
        let grant = grant(&storage.conn, &grant_request("^1")).unwrap();
        let revoked = revoke(&storage.conn, grant.id, 200).unwrap();
        assert_eq!(revoked.revoked_at, Some(200));
        assert!(active_for_plugin(&storage.conn, "com.example.test")
            .unwrap()
            .is_empty());
        assert_eq!(
            history_for_plugin(&storage.conn, "com.example.test")
                .unwrap()
                .len(),
            1
        );

        let audit = audit_for_plugin(&storage.conn, "com.example.test").unwrap();
        assert_eq!(
            audit
                .iter()
                .map(|entry| entry.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["granted", "revoked"]
        );
    }

    #[test]
    fn active_grants_can_be_granted_again_after_revocation() {
        let storage = Storage::open_in_memory().unwrap();
        let first = grant(&storage.conn, &grant_request("^1")).unwrap();
        revoke(&storage.conn, first.id, 200).unwrap();

        let mut next = grant_request("^1");
        next.granted_at = 300;
        let second = grant(&storage.conn, &next).unwrap();
        assert!(second.id > first.id);
        assert_eq!(
            active_for_plugin(&storage.conn, "com.example.test")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn invalid_version_ranges_are_rejected() {
        let storage = Storage::open_in_memory().unwrap();
        assert!(grant(&storage.conn, &grant_request("not a requirement")).is_err());
    }
}
