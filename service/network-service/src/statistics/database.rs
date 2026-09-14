//! SQLite persistence for historical traffic / health / diagnostics
//! (design doc §42). Schema migrations are additive.

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

use crate::common::SvcResult;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: &std::path::Path) -> SvcResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS sessions (
                id          TEXT PRIMARY KEY,
                node_id     TEXT NOT NULL,
                started_at  INTEGER NOT NULL,
                ended_at    INTEGER,
                up          INTEGER NOT NULL DEFAULT 0,
                down        INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS traffic_samples (
                ts          INTEGER NOT NULL,
                node_id     TEXT NOT NULL,
                up_delta    INTEGER NOT NULL,
                down_delta  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_samples_ts ON traffic_samples(ts);
            CREATE TABLE IF NOT EXISTS node_health (
                ts          INTEGER NOT NULL,
                node_id     TEXT NOT NULL,
                latency_ms  INTEGER NOT NULL,
                loss        REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS diagnostics (
                ts          INTEGER NOT NULL,
                name        TEXT NOT NULL,
                status      TEXT NOT NULL,
                latency_ms  INTEGER NOT NULL,
                detail      TEXT NOT NULL
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn begin_session(&self, id: &str, node_id: &str, ts: i64) -> SvcResult<()> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO sessions(id,node_id,started_at,up,down) VALUES (?1,?2,?3,0,0)",
            rusqlite::params![id, node_id, ts],
        )?;
        Ok(())
    }

    pub fn end_session(&self, id: &str, ts: i64, up: u64, down: u64) -> SvcResult<()> {
        self.conn.lock().execute(
            "UPDATE sessions SET ended_at=?2, up=?3, down=?4 WHERE id=?1",
            rusqlite::params![id, ts, up, down],
        )?;
        Ok(())
    }

    pub fn insert_traffic(&self, ts: i64, node_id: &str, up: u64, down: u64) -> SvcResult<()> {
        self.conn.lock().execute(
            "INSERT INTO traffic_samples(ts,node_id,up_delta,down_delta) VALUES (?1,?2,?3,?4)",
            rusqlite::params![ts, node_id, up, down],
        )?;
        Ok(())
    }

    pub fn insert_health(&self, ts: i64, node_id: &str, latency: i64, loss: f64) -> SvcResult<()> {
        self.conn.lock().execute(
            "INSERT INTO node_health(ts,node_id,latency_ms,loss) VALUES (?1,?2,?3,?4)",
            rusqlite::params![ts, node_id, latency, loss],
        )?;
        Ok(())
    }

    pub fn insert_diagnostic(
        &self,
        ts: i64,
        name: &str,
        status: &str,
        latency: i64,
        detail: &str,
    ) -> SvcResult<()> {
        self.conn.lock().execute(
            "INSERT INTO diagnostics(ts,name,status,latency_ms,detail) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![ts, name, status, latency, detail],
        )?;
        Ok(())
    }

    pub fn sum_since(&self, ts: i64) -> SvcResult<(u64, u64)> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COALESCE(SUM(up_delta),0), COALESCE(SUM(down_delta),0) \
             FROM traffic_samples WHERE ts >= ?1",
            rusqlite::params![ts],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
        )
        .map_err(Into::into)
    }

    /// Return per-sample up/down bytes in [start, end) for chart rendering.
    pub fn query_range(&self, start: i64, end: i64) -> SvcResult<Vec<(i64, u64, u64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, up_delta, down_delta FROM traffic_samples \
             WHERE ts >= ?1 AND ts < ?2 ORDER BY ts",
        )?;
        let rows = stmt.query_map(rusqlite::params![start, end], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64))
        })?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
}
