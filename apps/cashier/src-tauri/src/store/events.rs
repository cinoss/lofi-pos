use crate::error::{AppError, AppResult};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

/// `EventStore` uses an `r2d2_sqlite` pool for reads (WAL allows concurrent
/// readers) and a single `Mutex<Connection>` for the writer (rusqlite's
/// `Connection: !Sync`; SQLite serializes writes anyway). This shape lets
/// many parallel HTTP/Tauri handlers project state simultaneously while
/// `append` still maintains write-ordering invariants.
///
/// In-memory mode behaves differently: shared-cache memory SQLite databases
/// don't reliably support WAL (many builds silently fall back to MEMORY/DELETE
/// journal modes), which causes `SQLITE_BUSY` / "database table is locked"
/// errors when readers and the writer race. Tests don't need parallelism, so
/// in-memory mode collapses everything onto a single `Mutex<Connection>`.
pub struct EventStore {
    backend: Backend,
}

enum Backend {
    File {
        /// Pooled connections for read paths. WAL allows concurrent readers.
        read_pool: Pool<SqliteConnectionManager>,
        /// Single writer connection (rusqlite::Connection is !Sync).
        /// Append serializes through this mutex; reads bypass via the pool.
        writer: Mutex<Connection>,
    },
    /// Tests-only: shared-cache memory DBs can't run WAL reliably, so
    /// reads and writes share one mutex-guarded connection.
    Memory { conn: Mutex<Connection> },
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub business_day: String,
    pub ts: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub actor_staff: Option<i64>,
    pub actor_name: Option<String>,
    pub override_staff_id: Option<i64>,
    pub override_staff_name: Option<String>,
    pub payload_enc: Vec<u8>,
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct AppendEvent<'a> {
    pub business_day: &'a str,
    pub ts: i64,
    pub event_type: &'a str,
    pub aggregate_id: &'a str,
    pub actor_staff: Option<i64>,
    pub actor_name: Option<&'a str>,
    pub override_staff_id: Option<i64>,
    pub override_staff_name: Option<&'a str>,
    pub payload_enc: &'a [u8],
    pub key_id: &'a str,
}

impl EventStore {
    pub fn open(path: &Path) -> AppResult<Self> {
        // Run migrations on a one-shot connection first.
        let mut bootstrap = Connection::open(path)?;
        bootstrap.pragma_update(None, "journal_mode", "WAL")?;
        bootstrap.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut bootstrap,
            &crate::store::migrations::EVENTS_MIGRATIONS,
        )?;
        drop(bootstrap);

        // Build the read pool.
        let manager = SqliteConnectionManager::file(path).with_init(|c| {
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        });
        let read_pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .map_err(|e| AppError::Internal(format!("r2d2: {e}")))?;

        // Dedicated writer connection.
        let writer = Connection::open(path)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;

        Ok(Self {
            backend: Backend::File {
                read_pool,
                writer: Mutex::new(writer),
            },
        })
    }

    pub fn open_in_memory() -> AppResult<Self> {
        // Shared-cache memory URI so the bootstrap+migrations stay visible.
        // We only keep one connection for the whole store: WAL is unreliable
        // on memory DBs, and a single mutex sidesteps the lock contention.
        let uri = format!(
            "file:eventstore_mem_{}?mode=memory&cache=shared",
            uuid::Uuid::new_v4().simple()
        );
        let mut conn = Connection::open(&uri)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut conn,
            &crate::store::migrations::EVENTS_MIGRATIONS,
        )?;

        Ok(Self {
            backend: Backend::Memory {
                conn: Mutex::new(conn),
            },
        })
    }

    fn with_read<R>(&self, f: impl FnOnce(&Connection) -> AppResult<R>) -> AppResult<R> {
        match &self.backend {
            Backend::File { read_pool, .. } => {
                let conn = read_pool
                    .get()
                    .map_err(|e| AppError::Internal(format!("r2d2 get: {e}")))?;
                f(&conn)
            }
            Backend::Memory { conn } => {
                let guard = conn.lock().unwrap();
                f(&guard)
            }
        }
    }

    fn with_write<R>(&self, f: impl FnOnce(&Connection) -> AppResult<R>) -> AppResult<R> {
        match &self.backend {
            Backend::File { writer, .. } => {
                let guard = writer.lock().unwrap();
                f(&guard)
            }
            Backend::Memory { conn } => {
                let guard = conn.lock().unwrap();
                f(&guard)
            }
        }
    }

    pub fn append(&self, ev: AppendEvent<'_>) -> AppResult<i64> {
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO event
                 (business_day, ts, type, aggregate_id, actor_staff, actor_name, override_staff_id, override_staff_name, payload_enc, key_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    ev.business_day,
                    ev.ts,
                    ev.event_type,
                    ev.aggregate_id,
                    ev.actor_staff,
                    ev.actor_name,
                    ev.override_staff_id,
                    ev.override_staff_name,
                    ev.payload_enc,
                    ev.key_id
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn list_for_day(&self, business_day: &str) -> AppResult<Vec<EventRow>> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, business_day, ts, type, aggregate_id, actor_staff, actor_name, override_staff_id, override_staff_name, payload_enc, key_id
                 FROM event WHERE business_day = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt
                .query_map(params![business_day], row_to_event)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn list_for_aggregate(&self, aggregate_id: &str) -> AppResult<Vec<EventRow>> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, business_day, ts, type, aggregate_id, actor_staff, actor_name, override_staff_id, override_staff_name, payload_enc, key_id
                 FROM event WHERE aggregate_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt
                .query_map(params![aggregate_id], row_to_event)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn count_for_day(&self, business_day: &str) -> AppResult<i64> {
        self.with_read(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM event WHERE business_day = ?1",
                params![business_day],
                |r| r.get(0),
            )?)
        })
    }

    pub fn delete_day(&self, business_day: &str) -> AppResult<usize> {
        self.with_write(|conn| {
            let n = conn.execute(
                "DELETE FROM event WHERE business_day = ?1",
                params![business_day],
            )?;
            Ok(n)
        })
    }

    pub fn vacuum(&self) -> AppResult<()> {
        self.with_write(|conn| {
            conn.execute_batch("VACUUM")?;
            Ok(())
        })
    }

    /// List the distinct aggregate ids that have ever emitted an event of
    /// the given `event_type`. Used by command-service helpers to seed an
    /// "active" scan over a single event class (e.g. all SessionOpened ids).
    pub fn list_aggregate_ids_by_type(&self, event_type: &str) -> AppResult<Vec<String>> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT aggregate_id FROM event WHERE type = ?1 ORDER BY aggregate_id ASC",
            )?;
            let rows = stmt
                .query_map(params![event_type], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Aggregate ids that exist in the log but have no terminal event yet.
    /// Used by warm-up to limit replay to live aggregates only — closed
    /// sessions, paid sessions, merged sources, split sources, returned
    /// orders, etc., do NOT need their state in memory.
    pub fn list_live_aggregate_ids(&self) -> AppResult<Vec<String>> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT aggregate_id FROM event WHERE aggregate_id NOT IN (
                    SELECT aggregate_id FROM event
                    WHERE type IN ('SessionClosed','SessionSplit')
                ) ORDER BY aggregate_id ASC",
            )?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    /// Look up the most recent event for an aggregate, or None.
    pub fn latest_for_aggregate(&self, aggregate_id: &str) -> AppResult<Option<EventRow>> {
        self.with_read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, business_day, ts, type, aggregate_id, actor_staff, actor_name, override_staff_id, override_staff_name, payload_enc, key_id
                     FROM event WHERE aggregate_id = ?1 ORDER BY id DESC LIMIT 1",
                    params![aggregate_id],
                    row_to_event,
                )
                .optional()?)
        })
    }
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: r.get(0)?,
        business_day: r.get(1)?,
        ts: r.get(2)?,
        event_type: r.get(3)?,
        aggregate_id: r.get(4)?,
        actor_staff: r.get(5)?,
        actor_name: r.get(6)?,
        override_staff_id: r.get(7)?,
        override_staff_name: r.get(8)?,
        payload_enc: r.get(9)?,
        key_id: r.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(day: &'static str, agg: &'static str, ty: &'static str, ts: i64) -> AppendEvent<'static> {
        AppendEvent {
            business_day: day,
            ts,
            event_type: ty,
            aggregate_id: agg,
            actor_staff: Some(1),
            actor_name: None,
            override_staff_id: None,
            override_staff_name: None,
            payload_enc: b"ciphertext",
            key_id: day,
        }
    }

    #[test]
    fn append_then_list_for_day() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "sess-1", "SessionOpened", 100))
            .unwrap();
        s.append(ev("2026-04-27", "sess-1", "OrderPlaced", 200))
            .unwrap();
        let rows = s.list_for_day("2026-04-27").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event_type, "SessionOpened");
        assert_eq!(rows[1].event_type, "OrderPlaced");
        assert!(rows[0].id < rows[1].id);
    }

    #[test]
    fn list_for_aggregate_filters() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "sess-1", "SessionOpened", 100))
            .unwrap();
        s.append(ev("2026-04-27", "sess-2", "SessionOpened", 110))
            .unwrap();
        let rows = s.list_for_aggregate("sess-1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].aggregate_id, "sess-1");
    }

    #[test]
    fn delete_day_removes_only_that_day() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "a", "X", 1)).unwrap();
        s.append(ev("2026-04-28", "b", "Y", 2)).unwrap();
        let n = s.delete_day("2026-04-27").unwrap();
        assert_eq!(n, 1);
        assert_eq!(s.count_for_day("2026-04-27").unwrap(), 0);
        assert_eq!(s.count_for_day("2026-04-28").unwrap(), 1);
    }

    #[test]
    fn latest_for_aggregate_returns_newest() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "a", "X", 1)).unwrap();
        s.append(ev("2026-04-27", "a", "Y", 2)).unwrap();
        let last = s.latest_for_aggregate("a").unwrap().unwrap();
        assert_eq!(last.event_type, "Y");
    }

    #[test]
    fn list_aggregate_ids_by_type_distinct() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "sess-a", "SessionOpened", 1))
            .unwrap();
        s.append(ev("2026-04-27", "sess-a", "OrderPlaced", 2))
            .unwrap();
        s.append(ev("2026-04-27", "sess-b", "SessionOpened", 3))
            .unwrap();
        let ids = s.list_aggregate_ids_by_type("SessionOpened").unwrap();
        assert_eq!(ids, vec!["sess-a".to_string(), "sess-b".to_string()]);
    }

    #[test]
    fn list_live_aggregate_ids_excludes_closed() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "live", "SessionOpened", 1))
            .unwrap();
        s.append(ev("2026-04-27", "closed", "SessionOpened", 2))
            .unwrap();
        s.append(ev("2026-04-27", "closed", "SessionClosed", 3))
            .unwrap();
        let live = s.list_live_aggregate_ids().unwrap();
        assert_eq!(live, vec!["live"]);
    }

    #[test]
    fn latest_for_aggregate_returns_none_when_empty() {
        let s = EventStore::open_in_memory().unwrap();
        assert!(s.latest_for_aggregate("nope").unwrap().is_none());
    }
}
