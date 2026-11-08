use anyhow::{Context as _, Result};
use rusqlite::Connection;

use crate::ClipboardHistoryRow;

pub(crate) fn insert(conn: &Connection, text: &str, copied_at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO clipboard_history(text, copied_at) VALUES (?1, ?2)",
        (text, copied_at),
    )
    .context("insert clipboard history")?;
    Ok(())
}

pub(crate) fn recent(conn: &Connection, limit: i64) -> Result<Vec<ClipboardHistoryRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, text, copied_at FROM clipboard_history
             ORDER BY copied_at DESC, id DESC LIMIT ?1",
        )
        .context("prepare recent clipboard query")?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(ClipboardHistoryRow {
                id: row.get(0)?,
                text: row.get(1)?,
                copied_at: row.get(2)?,
            })
        })
        .context("query recent clipboard")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect recent clipboard")
}

pub(crate) fn trim(conn: &Connection, keep: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM clipboard_history WHERE id NOT IN (
            SELECT id FROM clipboard_history
            ORDER BY copied_at DESC, id DESC LIMIT ?1
        )",
        [keep],
    )
    .context("trim clipboard history")?;
    Ok(())
}

pub(crate) fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM clipboard_history WHERE id = ?1", (id,))
        .context("delete clipboard history")?;
    Ok(())
}
