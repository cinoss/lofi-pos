use crate::acl::Role;
use crate::error::{AppError, AppResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Staff {
    pub id: i64,
    pub name: String,
    pub pin_hash: String,
    pub role: Role,
    pub team: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpotKind {
    Room,
    Table,
}

impl SpotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpotKind::Room => "room",
            SpotKind::Table => "table",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "room" => Some(SpotKind::Room),
            "table" => Some(SpotKind::Table),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Spot {
    pub id: i64,
    pub name: String,
    pub kind: SpotKind,
    pub hourly_rate: Option<i64>,
    pub parent_id: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub price: i64,
    pub route: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecipeIngredient {
    pub ingredient_id: i64,
    pub ingredient_name: String,
    pub qty: f64,
    pub unit: String,
}

/// Connection wrapper for `master.db`. Holds CRUD-style state (staff, spots,
/// products, recipes, settings, idempotency, eod_runs, print_queue, denylist).
/// Single writer; caller serializes via outer mutex.
pub struct Master {
    conn: Connection,
}

impl Master {
    /// Open (or create) the on-disk master DB at `path`, enabling WAL +
    /// foreign keys, and running pending migrations.
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut conn,
            &crate::store::migrations::MASTER_MIGRATIONS,
        )?;
        Ok(Self { conn })
    }
    /// Open an in-memory master DB with migrations applied; intended for tests.
    pub fn open_in_memory() -> AppResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut conn,
            &crate::store::migrations::MASTER_MIGRATIONS,
        )?;
        Ok(Self { conn })
    }

    /// Read a key/value pair from the `setting` table.
    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }
    /// Run `f` inside a transaction, committing on Ok or rolling back on Err.
    pub fn with_tx<F, R>(&mut self, f: F) -> AppResult<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> AppResult<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }

    /// Upsert a key/value pair in the `setting` table.
    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO setting(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Insert a staff row. Returns the new id.
    pub fn create_staff(
        &self,
        name: &str,
        pin_hash: &str,
        role: Role,
        team: Option<&str>,
    ) -> AppResult<i64> {
        self.conn.execute(
            "INSERT INTO staff(name, pin_hash, role, team, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![name, pin_hash, role.as_str(), team, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_staff(&self, id: i64) -> AppResult<Option<Staff>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, pin_hash, role, team FROM staff WHERE id = ?1",
                params![id],
                row_to_staff,
            )
            .optional()?)
    }

    /// Update mutable fields on an existing staff row. Any of name/pin_hash/role/team
    /// may be `None` to leave that field untouched. Returns true on hit.
    pub fn update_staff(
        &self,
        id: i64,
        name: Option<&str>,
        pin_hash: Option<&str>,
        role: Option<Role>,
        team: Option<Option<&str>>,
    ) -> AppResult<bool> {
        // Defensive: read existing row, fold optional updates, write back.
        let existing = match self.get_staff(id)? {
            Some(s) => s,
            None => return Ok(false),
        };
        let new_name = name.unwrap_or(&existing.name);
        let new_pin = pin_hash.unwrap_or(&existing.pin_hash);
        let new_role = role.unwrap_or(existing.role);
        let new_team_owned: Option<String> = match team {
            Some(t) => t.map(|s| s.to_string()),
            None => existing.team,
        };
        let n = self.conn.execute(
            "UPDATE staff SET name = ?1, pin_hash = ?2, role = ?3, team = ?4 WHERE id = ?5",
            params![new_name, new_pin, new_role.as_str(), new_team_owned, id],
        )?;
        Ok(n > 0)
    }

    /// Delete a staff row.
    pub fn delete_staff(&self, id: i64) -> AppResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM staff WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// List all staff, ordered by id.
    pub fn list_staff(&self) -> AppResult<Vec<Staff>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, pin_hash, role, team FROM staff ORDER BY id ASC")?;
        let rows = stmt
            .query_map([], row_to_staff)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insert a spot row. Returns the new id.
    pub fn create_spot(
        &self,
        name: &str,
        kind: SpotKind,
        hourly_rate: Option<i64>,
        parent_id: Option<i64>,
    ) -> AppResult<i64> {
        if kind == SpotKind::Table && hourly_rate.is_some() {
            return Err(AppError::Validation("table cannot have hourly_rate".into()));
        }
        if kind == SpotKind::Room && hourly_rate.is_none() {
            return Err(AppError::Validation("room must have hourly_rate".into()));
        }
        self.conn.execute(
            "INSERT INTO spot(name, kind, hourly_rate, parent_id, status)
             VALUES(?1, ?2, ?3, ?4, 'idle')",
            params![name, kind.as_str(), hourly_rate, parent_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_spot(&self, id: i64) -> AppResult<Option<Spot>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, kind, hourly_rate, parent_id, status FROM spot WHERE id = ?1",
                params![id],
                row_to_spot,
            )
            .optional()?)
    }

    /// Update fields on an existing spot. Returns true on hit.
    pub fn update_spot(
        &self,
        id: i64,
        name: &str,
        kind: SpotKind,
        hourly_rate: Option<i64>,
        parent_id: Option<i64>,
    ) -> AppResult<bool> {
        if kind == SpotKind::Table && hourly_rate.is_some() {
            return Err(AppError::Validation("table cannot have hourly_rate".into()));
        }
        if kind == SpotKind::Room && hourly_rate.is_none() {
            return Err(AppError::Validation("room must have hourly_rate".into()));
        }
        let n = self.conn.execute(
            "UPDATE spot SET name = ?1, kind = ?2, hourly_rate = ?3, parent_id = ?4
             WHERE id = ?5",
            params![name, kind.as_str(), hourly_rate, parent_id, id],
        )?;
        Ok(n > 0)
    }

    /// Delete a spot row. Returns true if a row was removed.
    pub fn delete_spot(&self, id: i64) -> AppResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM spot WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// List all spots, ordered by id.
    pub fn list_spots(&self) -> AppResult<Vec<Spot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, hourly_rate, parent_id, status FROM spot ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_spot)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Look up a single product by id.
    pub fn get_product(&self, id: i64) -> AppResult<Option<Product>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, price, route, kind FROM product WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Product {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        price: r.get(2)?,
                        route: r.get(3)?,
                        kind: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Get recipe rows for `product_id`, joining the product table to resolve
    /// the ingredient name. Used at order-write time to snapshot the recipe
    /// into the event payload so historical reports reproduce.
    pub fn get_recipe(&self, product_id: i64) -> AppResult<Vec<RecipeIngredient>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.ingredient_id, p.name, r.qty, r.unit
             FROM recipe r
             JOIN product p ON p.id = r.ingredient_id
             WHERE r.product_id = ?1
             ORDER BY r.ingredient_id ASC",
        )?;
        let rows = stmt
            .query_map(params![product_id], |r| {
                Ok(RecipeIngredient {
                    ingredient_id: r.get(0)?,
                    ingredient_name: r.get(1)?,
                    qty: r.get(2)?,
                    unit: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insert a product row. Returns the new id.
    pub fn create_product(&self, name: &str, price: i64, route: &str, kind: &str) -> AppResult<i64> {
        self.conn.execute(
            "INSERT INTO product(name, price, route, kind) VALUES (?1, ?2, ?3, ?4)",
            params![name, price, route, kind],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update mutable fields on an existing product row. Returns true on hit.
    pub fn update_product(
        &self,
        id: i64,
        name: &str,
        price: i64,
        route: &str,
        kind: &str,
    ) -> AppResult<bool> {
        let n = self.conn.execute(
            "UPDATE product SET name = ?1, price = ?2, route = ?3, kind = ?4 WHERE id = ?5",
            params![name, price, route, kind, id],
        )?;
        Ok(n > 0)
    }

    /// Delete a product row.
    pub fn delete_product(&self, id: i64) -> AppResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM product WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// List all products, ordered by id.
    pub fn list_products(&self) -> AppResult<Vec<Product>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, price, route, kind FROM product ORDER BY id ASC")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Product {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    price: r.get(2)?,
                    route: r.get(3)?,
                    kind: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_idempotency(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT result_json FROM idempotency_key WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Insert; on conflict do nothing (caller treats absence of error as
    /// "stored or already-stored").
    // TODO(plan-e-eod): prune idempotency_key rows where created_at < now - N_DAYS
    // during the EOD pipeline. Today this table grows unbounded.
    pub fn put_idempotency(
        &self,
        key: &str,
        command: &str,
        result_json: &str,
        now_ms: i64,
    ) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO idempotency_key(key, command, result_json, created_at)
             VALUES(?1, ?2, ?3, ?4) ON CONFLICT(key) DO NOTHING",
            params![key, command, result_json, now_ms],
        )?;
        Ok(())
    }

    /// Add a token's `jti` to the denylist. Idempotent — re-revoking a token
    /// that's already on the list is a no-op (preserves the original
    /// `revoked_at`).
    pub fn put_token_denylist(&self, jti: &str, expires_at: i64, now_ms: i64) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO token_denylist(jti, expires_at, revoked_at)
             VALUES(?1, ?2, ?3) ON CONFLICT(jti) DO NOTHING",
            params![jti, expires_at, now_ms],
        )?;
        Ok(())
    }

    /// Returns true if the given `jti` has been revoked.
    pub fn is_token_denylisted(&self, jti: &str) -> AppResult<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM token_denylist WHERE jti = ?1",
                params![jti],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Plan F: read the `status` column from `eod_runs` for `business_day`.
    pub fn get_eod_runs_status(&self, business_day: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT status FROM eod_runs WHERE business_day = ?1",
                params![business_day],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Plan F: insert (or restart) an EOD run row, marking it 'running'.
    /// Used by the runner before it begins work; idempotent across retries.
    pub fn upsert_eod_running(&self, business_day: &str, started_at: i64) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO eod_runs(business_day, started_at, status, finished_at, error)
             VALUES (?1, ?2, 'running', NULL, NULL)
             ON CONFLICT(business_day) DO UPDATE SET
               started_at = excluded.started_at,
               status     = 'running',
               finished_at = NULL,
               error      = NULL",
            params![business_day, started_at],
        )?;
        Ok(())
    }

    /// Plan F: mark `business_day` as failed, recording the error message and
    /// finished_at. Used when build/write fails.
    pub fn set_eod_runs_failed(
        &self,
        business_day: &str,
        finished_at: i64,
        error: &str,
    ) -> AppResult<()> {
        self.conn.execute(
            "UPDATE eod_runs SET finished_at = ?1, status = 'failed', error = ?2
             WHERE business_day = ?3",
            params![finished_at, error, business_day],
        )?;
        Ok(())
    }

    /// All idempotency cache rows for a given business day. Used by warm-up
    /// to repopulate the in-memory cache after restart.
    ///
    /// We don't currently store business_day on idempotency_key. Two options:
    ///  (a) add business_day column + migration
    ///  (b) load ALL idempotency rows on startup
    /// Choosing (b) for now — at <10k rows/day this is a one-time cheap scan.
    /// The `business_day` arg is accepted for future API stability.
    pub fn list_idempotency_for_day(&self, business_day: &str) -> AppResult<Vec<(String, String)>> {
        let _ = business_day;
        let mut stmt = self
            .conn
            .prepare("SELECT key, result_json FROM idempotency_key")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- Print queue ----------

    /// Insert a new print job, ready to be picked up immediately.
    pub fn enqueue_print(
        &self,
        kind: &str,
        payload_json: &str,
        target: Option<&str>,
        now_ms: i64,
    ) -> AppResult<i64> {
        self.conn.execute(
            "INSERT INTO print_queue(kind, payload_json, target, attempts, last_error, enqueued_at, next_try_at)
             VALUES (?1, ?2, ?3, 0, NULL, ?4, ?4)",
            params![kind, payload_json, target, now_ms],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Pop the next eligible print job (smallest next_try_at &lt;= now). Returns
    /// None if nothing is ready.
    pub fn next_print_job(&self, now_ms: i64) -> AppResult<Option<PrintJob>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, kind, payload_json, target, attempts FROM print_queue
                 WHERE next_try_at <= ?1 ORDER BY id ASC LIMIT 1",
                params![now_ms],
                |r| {
                    Ok(PrintJob {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        payload_json: r.get(2)?,
                        target: r.get(3)?,
                        attempts: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn delete_print_job(&self, id: i64) -> AppResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM print_queue WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn reschedule_print_job(
        &self,
        id: i64,
        last_error: &str,
        next_try_at: i64,
    ) -> AppResult<()> {
        self.conn.execute(
            "UPDATE print_queue SET attempts = attempts + 1, last_error = ?2, next_try_at = ?3
             WHERE id = ?1",
            params![id, last_error, next_try_at],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PrintJob {
    pub id: i64,
    pub kind: String,
    pub payload_json: String,
    pub target: Option<String>,
    pub attempts: i64,
}

fn row_to_spot(r: &rusqlite::Row<'_>) -> rusqlite::Result<Spot> {
    let kind_str: String = r.get(2)?;
    let kind = SpotKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("bad spot kind: {kind_str}").into(),
        )
    })?;
    Ok(Spot {
        id: r.get(0)?,
        name: r.get(1)?,
        kind,
        hourly_rate: r.get(3)?,
        parent_id: r.get(4)?,
        status: r.get(5)?,
    })
}

fn row_to_staff(r: &rusqlite::Row<'_>) -> rusqlite::Result<Staff> {
    let role_str: String = r.get(3)?;
    let role = Role::parse(&role_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("bad role: {role_str}").into(),
        )
    })?;
    Ok(Staff {
        id: r.get(0)?,
        name: r.get(1)?,
        pin_hash: r.get(2)?,
        role,
        team: r.get(4)?,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_runs_migrations() {
        let m = Master::open_in_memory().unwrap();
        assert_eq!(
            m.get_setting("business_day_cutoff_hour")
                .unwrap()
                .as_deref(),
            Some("11")
        );
    }

    #[test]
    fn with_tx_commits_on_ok() {
        let mut m = Master::open_in_memory().unwrap();
        let r = m.with_tx(|tx| {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES('tx_test', 'a')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        assert_eq!(m.get_setting("tx_test").unwrap().as_deref(), Some("a"));
    }

    #[test]
    fn with_tx_rolls_back_on_err() {
        let mut m = Master::open_in_memory().unwrap();
        let r: AppResult<()> = m.with_tx(|tx| {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES('tx_rollback', 'x')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )?;
            Err(crate::error::AppError::Validation("force".into()))
        });
        assert!(r.is_err());
        assert!(m.get_setting("tx_rollback").unwrap().is_none());
    }

    #[test]
    fn list_idempotency_for_day_returns_all() {
        let m = Master::open_in_memory().unwrap();
        m.put_idempotency("k1", "cmd", "{\"a\":1}", 1).unwrap();
        m.put_idempotency("k2", "cmd", "{\"b\":2}", 2).unwrap();
        let rows = m.list_idempotency_for_day("2026-04-27").unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn token_denylist_put_then_check() {
        let m = Master::open_in_memory().unwrap();
        m.put_token_denylist("abc", 2_000_000, 1_000).unwrap();
        assert!(m.is_token_denylisted("abc").unwrap());
    }

    #[test]
    fn token_denylist_not_present_returns_false() {
        let m = Master::open_in_memory().unwrap();
        assert!(!m.is_token_denylisted("nope").unwrap());
    }

    #[test]
    fn token_denylist_put_is_idempotent() {
        let m = Master::open_in_memory().unwrap();
        m.put_token_denylist("dup", 2_000_000, 1_000).unwrap();
        // Second insert should not error; original row preserved.
        m.put_token_denylist("dup", 9_999_999, 5_000).unwrap();
        assert!(m.is_token_denylisted("dup").unwrap());
    }

    #[test]
    fn setting_upsert() {
        let m = Master::open_in_memory().unwrap();
        m.set_setting("x", "1").unwrap();
        m.set_setting("x", "2").unwrap();
        assert_eq!(m.get_setting("x").unwrap().as_deref(), Some("2"));
    }
}

#[cfg(test)]
mod staff_tests {
    use super::*;

    #[test]
    fn create_get_staff() {
        let m = Master::open_in_memory().unwrap();
        let id = m
            .create_staff("Alice", "hash1", Role::Cashier, Some("A"))
            .unwrap();
        let s = m.get_staff(id).unwrap().unwrap();
        assert_eq!(s.name, "Alice");
        assert_eq!(s.role, Role::Cashier);
        assert_eq!(s.team.as_deref(), Some("A"));
    }

    #[test]
    fn list_staff_empty() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.list_staff().unwrap().is_empty());
    }

    #[test]
    fn list_staff_ordered() {
        let m = Master::open_in_memory().unwrap();
        m.create_staff("Bob", "h", Role::Owner, None).unwrap();
        m.create_staff("Cara", "h", Role::Manager, None).unwrap();
        let v = m.list_staff().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Bob");
        assert_eq!(v[1].name, "Cara");
    }

    #[test]
    fn missing_staff_returns_none() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.get_staff(999).unwrap().is_none());
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn list_products_empty() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.list_products().unwrap().is_empty());
    }

    #[test]
    fn list_products_populated() {
        let m = Master::open_in_memory().unwrap();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Beer', 50000, 'bar', 'item')",
                [],
            )
            .unwrap();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Pho', 80000, 'kitchen', 'recipe')",
                [],
            )
            .unwrap();
        let v = m.list_products().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Beer");
        assert_eq!(v[0].route, "bar");
        assert_eq!(v[1].kind, "recipe");
    }
}

#[cfg(test)]
mod spot_tests {
    use super::*;

    #[test]
    fn create_room_with_rate_succeeds() {
        let m = Master::open_in_memory().unwrap();
        let id = m
            .create_spot("VIP-1", SpotKind::Room, Some(100_000), None)
            .unwrap();
        let s = m.get_spot(id).unwrap().unwrap();
        assert_eq!(s.kind, SpotKind::Room);
        assert_eq!(s.hourly_rate, Some(100_000));
        assert_eq!(s.name, "VIP-1");
    }

    #[test]
    fn create_table_with_rate_rejected() {
        let m = Master::open_in_memory().unwrap();
        let r = m.create_spot("T1", SpotKind::Table, Some(50_000), None);
        assert!(matches!(r, Err(AppError::Validation(_))));
    }

    #[test]
    fn create_room_without_rate_rejected() {
        let m = Master::open_in_memory().unwrap();
        let r = m.create_spot("R-bad", SpotKind::Room, None, None);
        assert!(matches!(r, Err(AppError::Validation(_))));
    }

    #[test]
    fn get_spot_by_id() {
        let m = Master::open_in_memory().unwrap();
        let id = m
            .create_spot("R1", SpotKind::Room, Some(60_000), None)
            .unwrap();
        let s = m.get_spot(id).unwrap().unwrap();
        assert_eq!(s.id, id);
        assert!(m.get_spot(99_999).unwrap().is_none());
    }

    #[test]
    fn list_spots_ordered() {
        let m = Master::open_in_memory().unwrap();
        let r1 = m
            .create_spot("R1", SpotKind::Room, Some(50_000), None)
            .unwrap();
        let _t1 = m
            .create_spot("T1", SpotKind::Table, None, Some(r1))
            .unwrap();
        let _r2 = m
            .create_spot("R2", SpotKind::Room, Some(80_000), None)
            .unwrap();
        let v = m.list_spots().unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "R1");
        assert_eq!(v[1].name, "T1");
        assert_eq!(v[1].kind, SpotKind::Table);
        assert_eq!(v[1].parent_id, Some(r1));
        assert_eq!(v[2].name, "R2");
    }
}

#[cfg(test)]
mod product_recipe_tests {
    use super::*;

    #[test]
    fn get_product_returns_none_for_missing_id() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.get_product(123).unwrap().is_none());
    }

    #[test]
    fn get_product_returns_full_row() {
        let m = Master::open_in_memory().unwrap();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Beer', 50000, 'bar', 'item')",
                [],
            )
            .unwrap();
        let id = m.conn.last_insert_rowid();
        let p = m.get_product(id).unwrap().unwrap();
        assert_eq!(p.name, "Beer");
        assert_eq!(p.price, 50_000);
        assert_eq!(p.route, "bar");
        assert_eq!(p.kind, "item");
    }

    #[test]
    fn get_recipe_returns_empty_for_no_recipe() {
        let m = Master::open_in_memory().unwrap();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Beer', 50000, 'bar', 'item')",
                [],
            )
            .unwrap();
        let id = m.conn.last_insert_rowid();
        assert!(m.get_recipe(id).unwrap().is_empty());
    }

    #[test]
    fn get_recipe_includes_ingredient_name() {
        let m = Master::open_in_memory().unwrap();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Pho', 80000, 'kitchen', 'recipe')",
                [],
            )
            .unwrap();
        let pho_id = m.conn.last_insert_rowid();
        m.conn
            .execute(
                "INSERT INTO product(name, price, route, kind) VALUES ('Noodle', 0, 'none', 'item')",
                [],
            )
            .unwrap();
        let noodle_id = m.conn.last_insert_rowid();
        m.conn
            .execute(
                "INSERT INTO recipe(product_id, ingredient_id, qty, unit) VALUES (?1, ?2, 200.0, 'g')",
                params![pho_id, noodle_id],
            )
            .unwrap();
        let r = m.get_recipe(pho_id).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ingredient_id, noodle_id);
        assert_eq!(r[0].ingredient_name, "Noodle");
        assert!((r[0].qty - 200.0).abs() < 1e-9);
        assert_eq!(r[0].unit, "g");
    }
}
