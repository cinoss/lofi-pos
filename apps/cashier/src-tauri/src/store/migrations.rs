use crate::error::{AppError, AppResult};
use include_dir::{include_dir, Dir};
use rusqlite::{params, Connection};

pub static MASTER_MIGRATIONS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations/master");
pub static EVENTS_MIGRATIONS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations/events");

pub fn run_migrations(conn: &mut Connection, dir: &Dir<'static>) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
             name TEXT PRIMARY KEY,
             applied_at INTEGER NOT NULL
         )",
    )?;

    let mut files: Vec<_> = dir
        .files()
        .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    files.sort_by_key(|f| f.path().to_owned());

    for file in files {
        let name = file
            .path()
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::Validation("bad migration filename".into()))?
            .to_string();
        let applied: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM _migrations WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .ok();
        if applied.is_some() {
            continue;
        }

        let sql = file
            .contents_utf8()
            .ok_or_else(|| AppError::Validation(format!("non-utf8 migration {name}")))?;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO _migrations(name, applied_at) VALUES (?1, ?2)",
            params![name, now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
