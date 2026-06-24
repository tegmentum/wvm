//! `Index` implementation backed by the `sqlite:wasm/high-level` component.
//!
//! Mirrors the native `rusqlite` schema and queries, but talks to the SQLite
//! component over the component-model interface.

use crate::sql;
use anyhow::{anyhow, Context, Result};
use wvm_core::index::{AppRecord, Index, Stats};
use wvm_core::manifest::Manifest;

/// Schema statements, applied one at a time (the high-level `execute` runs a
/// single statement).
const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS objects (digest TEXT PRIMARY KEY, size INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS versions (\
        id INTEGER PRIMARY KEY, runtime TEXT NOT NULL, version TEXT NOT NULL, \
        platform TEXT NOT NULL, archive_sha256 TEXT NOT NULL, \
        materialization TEXT NOT NULL, installed_at INTEGER NOT NULL, \
        UNIQUE(runtime, version))",
    "CREATE TABLE IF NOT EXISTS object_refs (\
        version_id INTEGER NOT NULL REFERENCES versions(id) ON DELETE CASCADE, \
        digest TEXT NOT NULL REFERENCES objects(digest), path TEXT NOT NULL, \
        mode TEXT NOT NULL, size INTEGER NOT NULL, PRIMARY KEY (version_id, path))",
    "CREATE INDEX IF NOT EXISTS idx_object_refs_digest ON object_refs(digest)",
    "CREATE TABLE IF NOT EXISTS apps (\
        name TEXT PRIMARY KEY, path TEXT, runtime_path TEXT, registered_at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS app_runtimes (\
        app TEXT NOT NULL REFERENCES apps(name) ON DELETE CASCADE, \
        version TEXT NOT NULL, PRIMARY KEY (app, version))",
    "CREATE INDEX IF NOT EXISTS idx_app_runtimes_version ON app_runtimes(version)",
];

pub struct ComponentIndex {
    conn: sql::Connection,
}

impl ComponentIndex {
    /// Open (creating if needed) the index DB at `db_path` and apply the schema.
    pub fn open(db_path: &str) -> Result<ComponentIndex> {
        let conn = sql::open_file(db_path).map_err(dberr)?;
        conn.execute("PRAGMA foreign_keys=ON").map_err(dberr)?;
        for stmt in SCHEMA {
            conn.execute(stmt)
                .map_err(dberr)
                .with_context(|| format!("applying schema: {stmt}"))?;
        }
        Ok(ComponentIndex { conn })
    }

    fn exec(&self, sql: &str, params: &[sql::Value]) -> Result<()> {
        self.conn.execute_with_params(sql, params).map_err(dberr)?;
        Ok(())
    }

    fn query(&self, sql: &str, params: &[sql::Value]) -> Result<sql::QueryResult> {
        self.conn.query_with_params(sql, params).map_err(dberr)
    }
}

impl Index for ComponentIndex {
    fn clear(&mut self) -> Result<()> {
        for stmt in [
            "DELETE FROM object_refs",
            "DELETE FROM versions",
            "DELETE FROM objects",
        ] {
            self.exec(stmt, &[])?;
        }
        Ok(())
    }

    fn upsert_object(&mut self, digest: &str, size: i64) -> Result<()> {
        self.exec(
            "INSERT INTO objects(digest, size) VALUES(?, ?) \
             ON CONFLICT(digest) DO UPDATE SET size=excluded.size",
            &[text(digest), int(size)],
        )
    }

    fn delete_object(&mut self, digest: &str) -> Result<()> {
        self.exec("DELETE FROM objects WHERE digest=?", &[text(digest)])
    }

    fn record_install(&mut self, manifest: &Manifest, installed_at: i64) -> Result<()> {
        self.exec("BEGIN", &[])?;
        let result = (|| {
            self.exec(
                "INSERT INTO versions(runtime, version, platform, archive_sha256, materialization, installed_at) \
                 VALUES(?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(runtime, version) DO UPDATE SET \
                    platform=excluded.platform, archive_sha256=excluded.archive_sha256, \
                    materialization=excluded.materialization, installed_at=excluded.installed_at",
                &[
                    text(&manifest.runtime),
                    text(&manifest.version),
                    text(&manifest.platform),
                    text(&manifest.archive_sha256),
                    text(&manifest.materialization),
                    int(installed_at),
                ],
            )?;
            let rows = self.query(
                "SELECT id FROM versions WHERE runtime=? AND version=?",
                &[text(&manifest.runtime), text(&manifest.version)],
            )?;
            let version_id = rows
                .rows
                .first()
                .and_then(|r| r.columns.first())
                .and_then(as_int)
                .ok_or_else(|| anyhow!("could not read version id"))?;

            self.exec(
                "DELETE FROM object_refs WHERE version_id=?",
                &[int(version_id)],
            )?;
            for f in &manifest.files {
                self.exec(
                    "INSERT INTO objects(digest, size) VALUES(?, ?) ON CONFLICT(digest) DO NOTHING",
                    &[text(&f.sha256), int(f.size as i64)],
                )?;
                self.exec(
                    "INSERT INTO object_refs(version_id, digest, path, mode, size) VALUES(?, ?, ?, ?, ?)",
                    &[int(version_id), text(&f.sha256), text(&f.path), text(&f.mode), int(f.size as i64)],
                )?;
            }
            Ok(())
        })();

        if result.is_ok() {
            self.exec("COMMIT", &[])?;
        } else {
            let _ = self.exec("ROLLBACK", &[]);
        }
        result
    }

    fn remove_version(&mut self, runtime: &str, version: &str) -> Result<()> {
        self.exec(
            "DELETE FROM versions WHERE runtime=? AND version=?",
            &[text(runtime), text(version)],
        )
    }

    fn unreferenced_objects(&self) -> Result<Vec<(String, i64)>> {
        let rows = self.query(
            "SELECT digest, size FROM objects \
             WHERE digest NOT IN (SELECT digest FROM object_refs) ORDER BY size DESC",
            &[],
        )?;
        Ok(digest_size_rows(&rows))
    }

    fn all_objects(&self) -> Result<Vec<(String, i64)>> {
        let rows = self.query("SELECT digest, size FROM objects ORDER BY size DESC", &[])?;
        Ok(digest_size_rows(&rows))
    }

    fn backlinks(&self, digest: &str) -> Result<Vec<(String, String)>> {
        let rows = self.query(
            "SELECT v.runtime, v.version FROM object_refs r \
             JOIN versions v ON v.id = r.version_id WHERE r.digest = ? \
             ORDER BY v.runtime, v.version",
            &[text(digest)],
        )?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| {
                let rt = r.columns.first().and_then(as_text)?;
                let ver = r.columns.get(1).and_then(as_text)?;
                Some((rt, ver))
            })
            .collect())
    }

    fn stats(&self) -> Result<Stats> {
        let rows = self.query(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM objects",
            &[],
        )?;
        let first = rows.rows.first();
        let objects = first
            .and_then(|r| r.columns.first())
            .and_then(as_int)
            .unwrap_or(0);
        let total_size = first
            .and_then(|r| r.columns.get(1))
            .and_then(as_int)
            .unwrap_or(0);
        let refs = self.query("SELECT COUNT(DISTINCT digest) FROM object_refs", &[])?;
        let referenced = refs
            .rows
            .first()
            .and_then(|r| r.columns.first())
            .and_then(as_int)
            .unwrap_or(0);
        Ok(Stats { objects, referenced, total_size })
    }

    fn register_app(
        &mut self,
        name: &str,
        path: Option<&str>,
        runtime_path: Option<&str>,
        runtimes: &[String],
        registered_at: i64,
    ) -> Result<()> {
        self.exec("BEGIN", &[])?;
        let result = (|| {
            // Replace any prior registration (cascade clears its runtimes).
            self.exec("DELETE FROM apps WHERE name=?", &[text(name)])?;
            self.exec(
                "INSERT INTO apps(name, path, runtime_path, registered_at) VALUES(?, ?, ?, ?)",
                &[text(name), opt_text(path), opt_text(runtime_path), int(registered_at)],
            )?;
            for v in runtimes {
                self.exec(
                    "INSERT INTO app_runtimes(app, version) VALUES(?, ?)",
                    &[text(name), text(v)],
                )?;
            }
            Ok(())
        })();
        if result.is_ok() {
            self.exec("COMMIT", &[])?;
        } else {
            let _ = self.exec("ROLLBACK", &[]);
        }
        result
    }

    fn unregister_app(&mut self, name: &str) -> Result<bool> {
        let existed = !self
            .query("SELECT 1 FROM apps WHERE name=?", &[text(name)])?
            .rows
            .is_empty();
        self.exec("DELETE FROM apps WHERE name=?", &[text(name)])?;
        Ok(existed)
    }

    fn list_apps(&self) -> Result<Vec<AppRecord>> {
        let rows = self.query(
            "SELECT name, path, runtime_path FROM apps ORDER BY name",
            &[],
        )?;
        let mut apps = Vec::new();
        for r in &rows.rows {
            let name = r.columns.first().and_then(as_text).unwrap_or_default();
            let path = r.columns.get(1).and_then(as_text);
            let runtime_path = r.columns.get(2).and_then(as_text);
            let vrows = self.query(
                "SELECT version FROM app_runtimes WHERE app=? ORDER BY version",
                &[text(&name)],
            )?;
            let runtimes = vrows
                .rows
                .iter()
                .filter_map(|v| v.columns.first().and_then(as_text))
                .collect();
            apps.push(AppRecord { name, path, runtime_path, runtimes });
        }
        Ok(apps)
    }

    fn apps_using(&self, version: &str) -> Result<Vec<String>> {
        let rows = self.query(
            "SELECT DISTINCT app FROM app_runtimes WHERE version=? ORDER BY app",
            &[text(version)],
        )?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| r.columns.first().and_then(as_text))
            .collect())
    }
}

// --- value helpers -------------------------------------------------------

fn text(s: &str) -> sql::Value {
    sql::Value::Text(s.to_string())
}

fn int(n: i64) -> sql::Value {
    sql::Value::Integer(n)
}

fn opt_text(s: Option<&str>) -> sql::Value {
    match s {
        Some(v) => sql::Value::Text(v.to_string()),
        None => sql::Value::Null,
    }
}

fn as_int(v: &sql::Value) -> Option<i64> {
    match v {
        sql::Value::Integer(n) => Some(*n),
        _ => None,
    }
}

fn as_text(v: &sql::Value) -> Option<String> {
    match v {
        sql::Value::Text(s) => Some(s.clone()),
        _ => None,
    }
}

fn digest_size_rows(result: &sql::QueryResult) -> Vec<(String, i64)> {
    result
        .rows
        .iter()
        .filter_map(|r| {
            let digest = r.columns.first().and_then(as_text)?;
            let size = r.columns.get(1).and_then(as_int).unwrap_or(0);
            Some((digest, size))
        })
        .collect()
}

fn dberr(e: sql::DatabaseError) -> anyhow::Error {
    anyhow!("sqlite error {} ({}): {}", e.code, e.extended_code, e.message)
}
