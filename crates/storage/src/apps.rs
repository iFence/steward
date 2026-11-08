use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use rusqlite::Connection;
use steward_core_engine::AppEntry;

use crate::{path_key, unix_seconds};

pub(crate) fn upsert_usage(conn: &Connection, path: &Path) -> Result<()> {
    let key = path_key(path);
    let now = unix_seconds();
    conn.execute(
        "INSERT INTO usage(path, count, last_used) VALUES (?1, 1, ?2)
         ON CONFLICT(path) DO UPDATE SET
            count = count + 1,
            last_used = excluded.last_used",
        (key, now),
    )
    .context("upsert usage")?;
    Ok(())
}

pub(crate) fn frequency(conn: &Connection, path: &Path) -> u32 {
    let key = path_key(path);
    conn.query_row("SELECT count FROM usage WHERE path = ?1", (key,), |row| {
        row.get::<_, u32>(0)
    })
    .unwrap_or(0)
}

pub(crate) fn cached(conn: &Connection) -> Result<Vec<AppEntry>> {
    let mut stmt = conn
        .prepare("SELECT path, name FROM apps")
        .context("prepare cached apps query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AppEntry {
                path: std::path::PathBuf::from(row.get::<_, String>(0)?),
                name: row.get(1)?,
            })
        })
        .context("query cached apps")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect cached apps")
}

pub(crate) fn mark_seen(conn: &mut Connection, apps: &[AppEntry]) -> Result<()> {
    let now = unix_seconds();
    let tx = conn.transaction().context("begin scan transaction")?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO apps(path, name, last_seen) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen",
            )
            .context("prepare app upsert")?;
        let mut seen: HashSet<String> = HashSet::with_capacity(apps.len());
        for app in apps {
            let key = path_key(&app.path);
            stmt.execute((&key, &app.name, now))
                .context("upsert app cache")?;
            seen.insert(key);
        }
        let existing = {
            let mut all = tx
                .prepare("SELECT path FROM apps")
                .context("prepare cached paths query")?;
            let paths = all
                .query_map([], |row| row.get::<_, String>(0))
                .context("query cached paths")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("collect cached paths")?;
            paths
        };
        let mut remove = tx
            .prepare("DELETE FROM apps WHERE path = ?1")
            .context("prepare stale app delete")?;
        for path in existing {
            if !seen.contains(&path) {
                remove.execute([&path]).context("delete stale cached app")?;
            }
        }
    }
    tx.commit().context("commit scan transaction")?;
    Ok(())
}
