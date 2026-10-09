//! Local persistence in SQLite: settings, metric samples, pulled container
//! logs (with FTS5 full-text search), per-container log cursors and issue
//! history. Synchronous by design — the Tauri layer calls it from a blocking
//! thread. No Tauri dependency.

pub mod archive;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use portside_core::archive::{owner_matches, pod_name_matches};
use portside_core::{logline::Level, Issue, LogRecord, Settings};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("settings are corrupt: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS node_samples (
  cluster TEXT NOT NULL,
  ts_ms   INTEGER NOT NULL,
  node    TEXT NOT NULL,
  cpu     REAL NOT NULL,
  mem     REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_node_samples ON node_samples (cluster, node, ts_ms);
CREATE INDEX IF NOT EXISTS idx_node_samples_ts ON node_samples (ts_ms);

CREATE TABLE IF NOT EXISTS pod_samples (
  cluster   TEXT NOT NULL,
  ts_ms     INTEGER NOT NULL,
  namespace TEXT NOT NULL,
  pod       TEXT NOT NULL,
  cpu       REAL NOT NULL,
  mem       REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pod_samples ON pod_samples (cluster, namespace, pod, ts_ms);
CREATE INDEX IF NOT EXISTS idx_pod_samples_ts ON pod_samples (ts_ms);

CREATE TABLE IF NOT EXISTS logs (
  id        INTEGER PRIMARY KEY,
  cluster   TEXT NOT NULL,
  ts_ns     INTEGER NOT NULL,
  namespace TEXT NOT NULL,
  pod       TEXT NOT NULL,
  container TEXT NOT NULL,
  level     INTEGER NOT NULL,
  message   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_logs_ts ON logs (cluster, ts_ns);
CREATE INDEX IF NOT EXISTS idx_logs_pod ON logs (cluster, namespace, pod, ts_ns);
CREATE INDEX IF NOT EXISTS idx_logs_level ON logs (cluster, level, ts_ns);

CREATE VIRTUAL TABLE IF NOT EXISTS logs_fts USING fts5 (
  message, content = 'logs', content_rowid = 'id'
);
CREATE TRIGGER IF NOT EXISTS logs_ai AFTER INSERT ON logs BEGIN
  INSERT INTO logs_fts (rowid, message) VALUES (new.id, new.message);
END;
CREATE TRIGGER IF NOT EXISTS logs_ad AFTER DELETE ON logs BEGIN
  INSERT INTO logs_fts (logs_fts, rowid, message) VALUES ('delete', old.id, old.message);
END;

CREATE TABLE IF NOT EXISTS log_cursors (
  cluster       TEXT NOT NULL,
  pod_uid       TEXT NOT NULL,
  container     TEXT NOT NULL,
  last_ts_ns    INTEGER NOT NULL,
  restart_count INTEGER NOT NULL,
  updated_ms    INTEGER NOT NULL,
  PRIMARY KEY (cluster, pod_uid, container)
);

-- Which workload each pod belonged to, so its stored logs stay findable by
-- workload after the pod (or the whole workload) is gone.
CREATE TABLE IF NOT EXISTS pod_owners (
  cluster       TEXT NOT NULL,
  namespace     TEXT NOT NULL,
  pod           TEXT NOT NULL,
  owner_kind    TEXT NOT NULL,
  owner_name    TEXT NOT NULL,
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms  INTEGER NOT NULL,
  PRIMARY KEY (cluster, namespace, pod)
);

CREATE TABLE IF NOT EXISTS issue_history (
  id            INTEGER PRIMARY KEY,
  cluster       TEXT NOT NULL,
  key           TEXT NOT NULL,
  severity      TEXT NOT NULL,
  category      TEXT NOT NULL,
  kind          TEXT NOT NULL,
  namespace     TEXT,
  name          TEXT NOT NULL,
  title         TEXT NOT NULL,
  detail        TEXT NOT NULL,
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms  INTEGER NOT NULL,
  resolved_ms   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_issue_open ON issue_history (cluster, resolved_ms, key);
CREATE INDEX IF NOT EXISTS idx_issue_seen ON issue_history (cluster, last_seen_ms);
";

const SETTINGS_KEY: &str = "settings";
/// Copy of a settings value that couldn't be parsed, kept so it can be recovered by hand.
pub const SETTINGS_UNREADABLE_KEY: &str = "settings.unreadable";

fn level_to_i(l: Level) -> i64 {
    match l {
        Level::Trace => 0,
        Level::Debug => 1,
        Level::Info => 2,
        Level::Warning => 3,
        Level::Error => 4,
    }
}

fn level_name(i: i64) -> &'static str {
    match i {
        0 => "trace",
        1 => "debug",
        3 => "warning",
        4 => "error",
        _ => "info",
    }
}

fn level_from_name(s: &str) -> Option<i64> {
    Level::from_str(s).map(level_to_i)
}

/// A log line ready to insert.
#[derive(Debug, Clone)]
pub struct NewLog {
    pub ts_ns: i64,
    pub namespace: String,
    pub pod: String,
    pub container: String,
    pub level: Level,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct LogCursor {
    pub last_ts_ns: i64,
    pub restart_count: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogQuery {
    /// Full-text search over messages (all words must match, prefix-matched).
    pub search: Option<String>,
    pub namespace: Option<String>,
    pub pod: Option<String>,
    pub container: Option<String>,
    /// Only pods of this workload (in `namespace`, which is then required),
    /// including pods that no longer exist.
    pub workload: Option<WorkloadRef>,
    /// Level names; empty = all.
    pub levels: Vec<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    /// Keyset pagination: only rows that sort after this one (older, or the
    /// same instant with a lower id). Pass the last row of the previous page.
    pub before_id: Option<i64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadRef {
    pub kind: String,
    pub name: String,
}

/// A pod that has lines in the store, alive or not.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogSource {
    pub namespace: String,
    pub pod: String,
    /// Recorded controller (ReplicaSets folded into their Deployment); `None`
    /// for pods only seen before owners were recorded.
    pub owner_kind: Option<String>,
    pub owner_name: Option<String>,
    pub lines: i64,
    pub errors: i64,
    pub warnings: i64,
    pub first_ms: i64,
    pub last_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub ts_ms: i64,
    pub cpu: f64,
    pub mem: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistogramBucket {
    pub bucket_ms: i64,
    pub trace: i64,
    pub debug: i64,
    pub info: i64,
    pub warning: i64,
    pub error: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodLogTotals {
    pub namespace: String,
    pub pod: String,
    pub errors: i64,
    pub warnings: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueHistoryEntry {
    pub id: i64,
    pub key: String,
    pub severity: String,
    pub category: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub title: String,
    pub detail: String,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub resolved_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    pub log_lines: i64,
    pub oldest_log_ms: Option<i64>,
    pub node_samples: i64,
    pub pod_samples: i64,
    pub issues_tracked: i64,
    pub db_bytes: i64,
}

/// Turn free text into a safe FTS5 query: every word quoted and prefix-matched,
/// all words required.
fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // --- settings ---------------------------------------------------------

    pub fn load_settings(&self) -> Result<Settings> {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [SETTINGS_KEY], |r| r.get(0))
            .optional()?;
        let mut settings: Settings = match raw {
            Some(json) => match serde_json::from_str(&json) {
                Ok(s) => s,
                Err(e) => {
                    // The caller falls back to defaults and the next save replaces
                    // the row, so keep what was there.
                    conn.execute(
                        "INSERT INTO settings (key, value) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        params![SETTINGS_UNREADABLE_KEY, json],
                    )?;
                    return Err(e.into());
                }
            },
            None => Settings::default(),
        };
        drop(conn);
        // Saved before credentials were encrypted: encrypt them now.
        let plaintext = portside_secrets::AVAILABLE
            && settings.secrets_mut().iter().any(|v| !v.is_empty() && !portside_secrets::is_protected(v));
        for v in settings.secrets_mut() {
            // One that can't be decrypted (another Windows account or computer)
            // is dropped: the user types it again.
            *v = portside_secrets::unprotect(v).unwrap_or_default();
        }
        settings.normalize();
        if plaintext {
            self.save_settings(&settings)?;
        }
        Ok(settings)
    }

    /// A random id for this install, created on first use. It marks what the
    /// app leaves on a cluster (file-browser helper pods) as its own.
    pub fn install_id(&self) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<String> =
            conn.query_row("SELECT value FROM settings WHERE key = 'install_id'", [], |r| r.get(0)).optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        let id = format!("{:016x}", h.finish());
        conn.execute("INSERT INTO settings (key, value) VALUES ('install_id', ?1)", params![id])?;
        Ok(id)
    }

    /// Credentials are encrypted where the platform can (see `portside-secrets`).
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let mut stored = settings.clone();
        for v in stored.secrets_mut().into_iter().filter(|v| !v.is_empty()) {
            if let Some(p) = portside_secrets::protect(v) {
                *v = p;
            }
        }
        let json = serde_json::to_string(&stored)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![SETTINGS_KEY, json],
        )?;
        Ok(())
    }

    // --- metrics ----------------------------------------------------------

    pub fn record_samples(
        &self,
        cluster: &str,
        ts_ms: i64,
        nodes: &[(String, f64, f64)],
        pods: &[(String, String, f64, f64)],
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut n = tx.prepare_cached(
                "INSERT INTO node_samples (cluster, ts_ms, node, cpu, mem) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (node, cpu, mem) in nodes {
                n.execute(params![cluster, ts_ms, node, cpu, mem])?;
            }
            let mut p = tx.prepare_cached(
                "INSERT INTO pod_samples (cluster, ts_ms, namespace, pod, cpu, mem) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (ns, pod, cpu, mem) in pods {
                p.execute(params![cluster, ts_ms, ns, pod, cpu, mem])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Samples averaged into `bucket_ms`-wide buckets so a week of 15 s polls
    /// doesn't ship tens of thousands of points to a sparkline.
    pub fn node_history(&self, cluster: &str, node: &str, since_ms: i64, bucket_ms: i64) -> Result<Vec<Sample>> {
        self.bucketed(
            "SELECT (ts_ms / ?4) * ?4 AS b, AVG(cpu), AVG(mem) FROM node_samples
             WHERE cluster = ?1 AND node = ?2 AND ts_ms >= ?3 GROUP BY b ORDER BY b",
            params![cluster, node, since_ms, bucket_ms.max(1)],
        )
    }

    pub fn pod_history(&self, cluster: &str, namespace: &str, pod: &str, since_ms: i64, bucket_ms: i64) -> Result<Vec<Sample>> {
        self.bucketed(
            "SELECT (ts_ms / ?5) * ?5 AS b, AVG(cpu), AVG(mem) FROM pod_samples
             WHERE cluster = ?1 AND namespace = ?2 AND pod = ?3 AND ts_ms >= ?4 GROUP BY b ORDER BY b",
            params![cluster, namespace, pod, since_ms, bucket_ms.max(1)],
        )
    }

    /// Whole-cluster usage: node samples summed per poll, then bucket-averaged.
    pub fn cluster_history(&self, cluster: &str, since_ms: i64, bucket_ms: i64) -> Result<Vec<Sample>> {
        self.bucketed(
            "SELECT (ts_ms / ?3) * ?3 AS b, AVG(cpu), AVG(mem) FROM (
               SELECT ts_ms, SUM(cpu) AS cpu, SUM(mem) AS mem FROM node_samples
               WHERE cluster = ?1 AND ts_ms >= ?2 GROUP BY ts_ms
             ) GROUP BY b ORDER BY b",
            params![cluster, since_ms, bucket_ms.max(1)],
        )
    }

    fn bucketed(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Sample>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(Sample { ts_ms: r.get(0)?, cpu: r.get(1)?, mem: r.get(2)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- logs -------------------------------------------------------------

    pub fn get_cursor(&self, cluster: &str, pod_uid: &str, container: &str) -> Result<Option<LogCursor>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT last_ts_ns, restart_count FROM log_cursors
                 WHERE cluster = ?1 AND pod_uid = ?2 AND container = ?3",
                params![cluster, pod_uid, container],
                |r| Ok(LogCursor { last_ts_ns: r.get(0)?, restart_count: r.get(1)? }),
            )
            .optional()?)
    }

    /// Insert a batch of lines and advance the container's cursor atomically,
    /// so a crash mid-pull can't duplicate or drop lines.
    pub fn append_logs(
        &self,
        cluster: &str,
        pod_uid: &str,
        container: &str,
        cursor: LogCursor,
        lines: &[NewLog],
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO logs (cluster, ts_ns, namespace, pod, container, level, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for l in lines {
                ins.execute(params![
                    cluster,
                    l.ts_ns,
                    l.namespace,
                    l.pod,
                    l.container,
                    level_to_i(l.level),
                    l.message
                ])?;
            }
            tx.execute(
                "INSERT INTO log_cursors (cluster, pod_uid, container, last_ts_ns, restart_count, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (cluster, pod_uid, container) DO UPDATE SET
                   last_ts_ns = excluded.last_ts_ns,
                   restart_count = excluded.restart_count,
                   updated_ms = excluded.updated_ms",
                params![cluster, pod_uid, container, cursor.last_ts_ns, cursor.restart_count, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Build the shared WHERE clause for log queries. Returns (sql, params).
    /// `pods` is the workload filter already resolved to pod names.
    fn log_filter(cluster: &str, q: &LogQuery, pods: Option<&[String]>) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut sql = String::from("l.cluster = ?");
        let mut p: Vec<Value> = vec![Value::Text(cluster.into())];
        if let Some(fts) = q.search.as_deref().and_then(fts_query) {
            sql.push_str(" AND l.id IN (SELECT rowid FROM logs_fts WHERE logs_fts MATCH ?)");
            p.push(Value::Text(fts));
        }
        for (col, val) in [("namespace", &q.namespace), ("pod", &q.pod), ("container", &q.container)] {
            if let Some(v) = val.as_deref().filter(|v| !v.is_empty()) {
                sql.push_str(&format!(" AND l.{col} = ?"));
                p.push(Value::Text(v.into()));
            }
        }
        if let Some(pods) = pods {
            if pods.is_empty() {
                sql.push_str(" AND 0");
            } else {
                sql.push_str(&format!(" AND l.pod IN ({})", vec!["?"; pods.len()].join(",")));
                p.extend(pods.iter().map(|n| Value::Text(n.clone())));
            }
        }
        let levels: Vec<i64> = q.levels.iter().filter_map(|l| level_from_name(l)).collect();
        if !levels.is_empty() {
            sql.push_str(&format!(" AND l.level IN ({})", vec!["?"; levels.len()].join(",")));
            p.extend(levels.into_iter().map(Value::Integer));
        }
        if let Some(s) = q.since_ms {
            sql.push_str(" AND l.ts_ns >= ?");
            p.push(Value::Integer(s * 1_000_000));
        }
        if let Some(u) = q.until_ms {
            sql.push_str(" AND l.ts_ns < ?");
            p.push(Value::Integer(u * 1_000_000));
        }
        (sql, p)
    }

    /// Newest-first page of log lines, by the lines' own timestamps. (Ids
    /// follow pull order: a pod seen for the first time inserts a day of old
    /// lines after everyone else's recent ones.)
    pub fn query_logs(&self, cluster: &str, q: &LogQuery) -> Result<Vec<LogRecord>> {
        use rusqlite::types::Value;
        let conn = self.conn.lock().unwrap();
        let pods = Self::query_pods(&conn, cluster, q)?;
        let (mut filter, mut p) = Self::log_filter(cluster, q, pods.as_deref());
        if let Some(b) = q.before_id {
            filter.push_str(" AND (l.ts_ns, l.id) < (SELECT ts_ns, id FROM logs WHERE id = ?)");
            p.push(Value::Integer(b));
        }
        let limit = q.limit.unwrap_or(500).min(5000);
        let sql = format!(
            "SELECT l.id, l.ts_ns, l.namespace, l.pod, l.container, l.level, l.message
             FROM logs l WHERE {filter} ORDER BY l.ts_ns DESC, l.id DESC LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(p), |r| {
            Ok(LogRecord {
                id: r.get(0)?,
                ts_ms: r.get::<_, i64>(1)? / 1_000_000,
                namespace: r.get(2)?,
                pod: r.get(3)?,
                container: r.get(4)?,
                level: level_name(r.get(5)?).into(),
                message: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Log volume per level in `bucket_ms` buckets, honoring the same filters
    /// as [`Store::query_logs`].
    pub fn log_histogram(&self, cluster: &str, q: &LogQuery, bucket_ms: i64) -> Result<Vec<HistogramBucket>> {
        let conn = self.conn.lock().unwrap();
        let pods = Self::query_pods(&conn, cluster, q)?;
        let (filter, filter_params) = Self::log_filter(cluster, q, pods.as_deref());
        let bucket_ns = rusqlite::types::Value::Integer(bucket_ms.max(1) * 1_000_000);
        // The bucket width binds first (twice, in the SELECT), then the filter.
        let mut p = vec![bucket_ns.clone(), bucket_ns];
        p.extend(filter_params);
        let sql = format!(
            "SELECT (l.ts_ns / ?) * ? AS b, l.level, COUNT(*) FROM logs l
             WHERE {filter} GROUP BY b, l.level ORDER BY b"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(p))?;
        let mut out: Vec<HistogramBucket> = Vec::new();
        while let Some(r) = rows.next()? {
            let b: i64 = r.get::<_, i64>(0)? / 1_000_000;
            let level: i64 = r.get(1)?;
            let count: i64 = r.get(2)?;
            if out.last().map(|x| x.bucket_ms) != Some(b) {
                out.push(HistogramBucket { bucket_ms: b, ..Default::default() });
            }
            let e = out.last_mut().unwrap();
            match level {
                0 => e.trace += count,
                1 => e.debug += count,
                3 => e.warning += count,
                4 => e.error += count,
                _ => e.info += count,
            }
        }
        Ok(out)
    }

    /// The workload filter of `q` resolved to pod names (`None` = no filter).
    fn query_pods(conn: &Connection, cluster: &str, q: &LogQuery) -> Result<Option<Vec<String>>> {
        let Some(w) = &q.workload else { return Ok(None) };
        let ns = q
            .namespace
            .as_deref()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| StoreError::Invalid("a workload filter needs a namespace".into()))?;
        Ok(Some(Self::workload_pods(conn, cluster, ns, &w.kind, &w.name)?))
    }

    /// Every pod of a workload that has ever been seen: pods with a recorded
    /// owner, plus (for lines stored before owners were recorded) unowned
    /// pods whose name has the controller's shape.
    fn workload_pods(conn: &Connection, cluster: &str, namespace: &str, kind: &str, name: &str) -> Result<Vec<String>> {
        let mut known = HashSet::new();
        let mut out = Vec::new();
        let mut stmt = conn.prepare_cached("SELECT pod, owner_kind, owner_name FROM pod_owners WHERE cluster = ?1 AND namespace = ?2")?;
        let mut rows = stmt.query(params![cluster, namespace])?;
        while let Some(r) = rows.next()? {
            let (pod, ok, on): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            if owner_matches(kind, name, &ok, &on) {
                out.push(pod.clone());
            }
            known.insert(pod);
        }
        let mut stmt = conn.prepare_cached("SELECT DISTINCT pod FROM logs WHERE cluster = ?1 AND namespace = ?2")?;
        let mut rows = stmt.query(params![cluster, namespace])?;
        while let Some(r) = rows.next()? {
            let pod: String = r.get(0)?;
            if !known.contains(&pod) && pod_name_matches(kind, name, &pod) {
                out.push(pod);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Remember which workload each live pod belongs to. `pods` is
    /// (namespace, pod, owner kind, owner name).
    pub fn record_pod_owners(&self, cluster: &str, now_ms: i64, pods: &[(String, String, String, String)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut up = tx.prepare_cached(
                "INSERT INTO pod_owners (cluster, namespace, pod, owner_kind, owner_name, first_seen_ms, last_seen_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT (cluster, namespace, pod) DO UPDATE SET
                   owner_kind = excluded.owner_kind,
                   owner_name = excluded.owner_name,
                   last_seen_ms = excluded.last_seen_ms",
            )?;
            for (ns, pod, kind, name) in pods {
                up.execute(params![cluster, ns, pod, kind, name, now_ms])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Pods with stored lines, newest activity first, optionally limited to
    /// one namespace and/or one workload. Includes pods that no longer exist.
    pub fn log_sources(&self, cluster: &str, namespace: Option<&str>, workload: Option<&WorkloadRef>) -> Result<Vec<LogSource>> {
        use rusqlite::types::Value;
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT l.namespace, l.pod, COUNT(*), SUM(l.level = 4), SUM(l.level = 3), MIN(l.ts_ns), MAX(l.ts_ns),
                    o.owner_kind, o.owner_name
             FROM logs l LEFT JOIN pod_owners o
               ON o.cluster = l.cluster AND o.namespace = l.namespace AND o.pod = l.pod
             WHERE l.cluster = ?",
        );
        let mut p: Vec<Value> = vec![Value::Text(cluster.into())];
        if let Some(ns) = namespace.filter(|n| !n.is_empty()) {
            sql.push_str(" AND l.namespace = ?");
            p.push(Value::Text(ns.into()));
        }
        let q = LogQuery { namespace: namespace.map(str::to_string), workload: workload.cloned(), ..Default::default() };
        if let Some(pods) = Self::query_pods(&conn, cluster, &q)? {
            if pods.is_empty() {
                return Ok(Vec::new());
            }
            sql.push_str(&format!(" AND l.pod IN ({})", vec!["?"; pods.len()].join(",")));
            p.extend(pods.into_iter().map(Value::Text));
        }
        sql.push_str(" GROUP BY l.namespace, l.pod ORDER BY MAX(l.ts_ns) DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(p), |r| {
            Ok(LogSource {
                namespace: r.get(0)?,
                pod: r.get(1)?,
                lines: r.get(2)?,
                errors: r.get(3)?,
                warnings: r.get(4)?,
                first_ms: r.get::<_, i64>(5)? / 1_000_000,
                last_ms: r.get::<_, i64>(6)? / 1_000_000,
                owner_kind: r.get(7)?,
                owner_name: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Write every stored line of a workload, oldest first, as
    /// `<RFC 3339 time> <LEVEL> <pod>/<container> <message>`. Pages through the
    /// table so the lock isn't held for the whole export. Returns the line count.
    pub fn write_workload_logs(
        &self,
        cluster: &str,
        namespace: &str,
        workload: &WorkloadRef,
        out: &mut dyn std::io::Write,
    ) -> Result<usize> {
        use rusqlite::types::Value;
        const PAGE: usize = 5_000;
        let pods = {
            let conn = self.conn.lock().unwrap();
            Self::workload_pods(&conn, cluster, namespace, &workload.kind, &workload.name)?
        };
        if pods.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "SELECT id, ts_ns, pod, container, level, message FROM logs
             WHERE cluster = ? AND namespace = ? AND pod IN ({}) AND (ts_ns, id) > (?, ?)
             ORDER BY ts_ns, id LIMIT {PAGE}",
            vec!["?"; pods.len()].join(",")
        );
        let (mut after_ts, mut after_id, mut written) = (i64::MIN, i64::MIN, 0usize);
        loop {
            let page: Vec<(i64, i64, String, String, i64, String)> = {
                let conn = self.conn.lock().unwrap();
                let mut p: Vec<Value> = vec![Value::Text(cluster.into()), Value::Text(namespace.into())];
                p.extend(pods.iter().map(|n| Value::Text(n.clone())));
                p.push(Value::Integer(after_ts));
                p.push(Value::Integer(after_id));
                let mut stmt = conn.prepare_cached(&sql)?;
                let rows = stmt.query_map(params_from_iter(p), |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (id, ts, pod, container, level, message) in &page {
                let time = portside_core::jiff::Timestamp::from_nanosecond(*ts as i128)
                    .map(|t| t.to_string())
                    .unwrap_or_else(|_| ts.to_string());
                writeln!(out, "{time} {:<7} {pod}/{container} {message}", level_name(*level).to_ascii_uppercase())?;
                (after_ts, after_id) = (*ts, *id);
            }
            written += page.len();
            if page.len() < PAGE {
                return Ok(written);
            }
        }
    }

    /// Pods ranked by error lines since `since_ms`.
    pub fn top_pods_by_errors(&self, cluster: &str, since_ms: i64, limit: u32) -> Result<Vec<PodLogTotals>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT namespace, pod,
                    SUM(level = 4), SUM(level = 3), COUNT(*)
             FROM logs WHERE cluster = ?1 AND ts_ns >= ?2
             GROUP BY namespace, pod
             HAVING SUM(level >= 3) > 0
             ORDER BY SUM(level = 4) DESC, SUM(level = 3) DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![cluster, since_ms * 1_000_000, limit], |r| {
            Ok(PodLogTotals {
                namespace: r.get(0)?,
                pod: r.get(1)?,
                errors: r.get(2)?,
                warnings: r.get(3)?,
                total: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Per-pod error/warning counts since `since_ms`, for the pod table.
    pub fn pod_log_counts(&self, cluster: &str, since_ms: i64) -> Result<Vec<PodLogTotals>> {
        self.top_pods_by_errors(cluster, since_ms, u32::MAX)
    }

    /// Repeated error messages, grouped by their first 120 chars with digits
    /// collapsed, so "timeout after 3012ms" and "timeout after 2998ms" count
    /// as one pattern.
    pub fn top_error_patterns(&self, cluster: &str, since_ms: i64, limit: usize) -> Result<Vec<ErrorPattern>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT namespace, pod, message, ts_ns FROM logs
             WHERE cluster = ?1 AND level = 4 AND ts_ns >= ?2
             ORDER BY id DESC LIMIT 20000",
        )?;
        let mut groups: std::collections::HashMap<String, ErrorPattern> = Default::default();
        let mut rows = stmt.query(params![cluster, since_ms * 1_000_000])?;
        while let Some(r) = rows.next()? {
            let msg: String = r.get(2)?;
            let pattern = normalize_pattern(&msg);
            let ns: String = r.get(0)?;
            let pod: String = r.get(1)?;
            let ts_ms = r.get::<_, i64>(3)? / 1_000_000;
            let e = groups.entry(pattern.clone()).or_insert_with(|| ErrorPattern {
                pattern,
                sample: msg.clone(),
                count: 0,
                pods: Vec::new(),
                last_ms: ts_ms,
            });
            e.count += 1;
            let label = format!("{ns}/{pod}");
            if e.pods.len() < 5 && !e.pods.contains(&label) {
                e.pods.push(label);
            }
        }
        let mut out: Vec<ErrorPattern> = groups.into_values().collect();
        out.sort_by(|a, b| b.count.cmp(&a.count));
        out.truncate(limit);
        Ok(out)
    }

    // --- issues -----------------------------------------------------------

    /// Reconcile the current issue list with history: new keys open a row,
    /// existing open rows are touched, open rows not in `issues` get resolved.
    /// Fills `first_seen_ms` on each issue.
    pub fn sync_issues(&self, cluster: &str, now_ms: i64, issues: &mut [Issue]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let open: std::collections::HashMap<String, (i64, i64)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT key, id, first_seen_ms FROM issue_history
                 WHERE cluster = ?1 AND resolved_ms IS NULL",
            )?;
            let rows = stmt.query_map([cluster], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut seen = std::collections::HashSet::new();
        for issue in issues.iter_mut() {
            seen.insert(issue.key.clone());
            match open.get(&issue.key) {
                Some((id, first)) => {
                    tx.execute(
                        "UPDATE issue_history SET last_seen_ms = ?2, severity = ?3, title = ?4, detail = ?5 WHERE id = ?1",
                        params![id, now_ms, issue.severity.as_str(), issue.title, issue.detail],
                    )?;
                    issue.first_seen_ms = Some(*first);
                }
                None => {
                    tx.execute(
                        "INSERT INTO issue_history
                           (cluster, key, severity, category, kind, namespace, name, title, detail, first_seen_ms, last_seen_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                        params![
                            cluster,
                            issue.key,
                            issue.severity.as_str(),
                            issue.category,
                            issue.kind,
                            issue.namespace,
                            issue.name,
                            issue.title,
                            issue.detail,
                            now_ms
                        ],
                    )?;
                    issue.first_seen_ms = Some(now_ms);
                }
            }
        }
        for (key, (id, _)) in &open {
            if !seen.contains(key) {
                tx.execute("UPDATE issue_history SET resolved_ms = ?2 WHERE id = ?1", params![id, now_ms])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn issue_history(&self, cluster: &str, since_ms: i64, limit: u32) -> Result<Vec<IssueHistoryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, key, severity, category, kind, namespace, name, title, detail,
                    first_seen_ms, last_seen_ms, resolved_ms
             FROM issue_history WHERE cluster = ?1 AND last_seen_ms >= ?2
             ORDER BY (resolved_ms IS NULL) DESC, last_seen_ms DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![cluster, since_ms, limit], |r| {
            Ok(IssueHistoryEntry {
                id: r.get(0)?,
                key: r.get(1)?,
                severity: r.get(2)?,
                category: r.get(3)?,
                kind: r.get(4)?,
                namespace: r.get(5)?,
                name: r.get(6)?,
                title: r.get(7)?,
                detail: r.get(8)?,
                first_seen_ms: r.get(9)?,
                last_seen_ms: r.get(10)?,
                resolved_ms: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- housekeeping -----------------------------------------------------

    /// Delete everything older than `cutoff_ms` across all clusters. Returns
    /// the number of log lines removed.
    pub fn prune(&self, cutoff_ms: i64) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let logs = tx.execute("DELETE FROM logs WHERE ts_ns < ?1", [cutoff_ms * 1_000_000])?;
        tx.execute("DELETE FROM node_samples WHERE ts_ms < ?1", [cutoff_ms])?;
        tx.execute("DELETE FROM pod_samples WHERE ts_ms < ?1", [cutoff_ms])?;
        tx.execute(
            "DELETE FROM issue_history WHERE resolved_ms IS NOT NULL AND resolved_ms < ?1",
            [cutoff_ms],
        )?;
        tx.execute("DELETE FROM log_cursors WHERE updated_ms < ?1", [cutoff_ms])?;
        tx.execute("DELETE FROM pod_owners WHERE last_seen_ms < ?1", [cutoff_ms])?;
        tx.commit()?;
        Ok(logs)
    }

    /// Drop all stored logs, samples and issue history for one cluster.
    pub fn clear_cluster(&self, cluster: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for table in ["logs", "node_samples", "pod_samples", "log_cursors", "issue_history", "pod_owners"] {
            tx.execute(&format!("DELETE FROM {table} WHERE cluster = ?1"), [cluster])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn stats(&self, cluster: &str) -> Result<StorageStats> {
        let conn = self.conn.lock().unwrap();
        let count = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(sql, [cluster], |r| r.get(0)) };
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(StorageStats {
            log_lines: count("SELECT COUNT(*) FROM logs WHERE cluster = ?1")?,
            oldest_log_ms: conn
                .query_row("SELECT MIN(ts_ns) FROM logs WHERE cluster = ?1", [cluster], |r| {
                    r.get::<_, Option<i64>>(0)
                })?
                .map(|n| n / 1_000_000),
            node_samples: count("SELECT COUNT(*) FROM node_samples WHERE cluster = ?1")?,
            pod_samples: count("SELECT COUNT(*) FROM pod_samples WHERE cluster = ?1")?,
            issues_tracked: count("SELECT COUNT(*) FROM issue_history WHERE cluster = ?1")?,
            db_bytes: page_count * page_size,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPattern {
    pub pattern: String,
    pub sample: String,
    pub count: i64,
    pub pods: Vec<String>,
    pub last_ms: i64,
}

fn normalize_pattern(msg: &str) -> String {
    let mut out = String::with_capacity(120);
    let mut last_digit = false;
    for c in msg.chars().take(160) {
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
        } else {
            out.push(c);
            last_digit = false;
        }
        if out.len() >= 120 {
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use portside_core::{Severity};

    fn log(ts_ns: i64, level: Level, msg: &str) -> NewLog {
        NewLog {
            ts_ns,
            namespace: "default".into(),
            pod: "api".into(),
            container: "app".into(),
            level,
            message: msg.into(),
        }
    }

    #[test]
    fn logs_roundtrip_search_and_histogram() {
        let s = Store::open_in_memory().unwrap();
        let cur = LogCursor { last_ts_ns: 3_000_000_000, restart_count: 0 };
        s.append_logs(
            "c",
            "uid",
            "app",
            cur,
            &[
                log(1_000_000_000, Level::Info, "server started on port 8080"),
                log(2_000_000_000, Level::Error, "database connection refused"),
                log(3_000_000_000, Level::Warning, "slow query 812ms"),
            ],
            0,
        )
        .unwrap();

        let all = s.query_logs("c", &LogQuery::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].message, "slow query 812ms"); // newest first

        let hit = s
            .query_logs("c", &LogQuery { search: Some("connect refus".into()), ..Default::default() })
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].level, "error");

        let errs = s
            .query_logs("c", &LogQuery { levels: vec!["error".into()], ..Default::default() })
            .unwrap();
        assert_eq!(errs.len(), 1);

        let hist = s.log_histogram("c", &LogQuery::default(), 10_000).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!((hist[0].info, hist[0].warning, hist[0].error), (1, 1, 1));

        let c = s.get_cursor("c", "uid", "app").unwrap().unwrap();
        assert_eq!(c.last_ts_ns, 3_000_000_000);

        // prune removes from FTS too
        assert_eq!(s.prune(10_000).unwrap(), 3);
        assert!(s
            .query_logs("c", &LogQuery { search: Some("database".into()), ..Default::default() })
            .unwrap()
            .is_empty());
    }

    fn pod_log(pod: &str, ts_ns: i64, level: Level, msg: &str) -> NewLog {
        NewLog { pod: pod.into(), namespace: "apps".into(), ..log(ts_ns, level, msg) }
    }

    #[test]
    fn workload_logs_outlive_their_pods() {
        let s = Store::open_in_memory().unwrap();
        let cur = LogCursor { last_ts_ns: 0, restart_count: 0 };
        for (uid, pod, ts, lvl) in [
            ("1", "web-7d9f8b6c5-aaaaa", 1_000_000_000, Level::Info), // owner recorded
            ("2", "web-5c8d7f9b4-bbbbb", 2_000_000_000, Level::Error), // stored before owners existed
            ("3", "web-api-6f7b9c8d4-ccccc", 3_000_000_000, Level::Info), // a different Deployment
            ("4", "worker-0", 4_000_000_000, Level::Info),
        ] {
            s.append_logs("c", uid, "app", cur, &[pod_log(pod, ts, lvl, &format!("hello from {pod}"))], 0).unwrap();
        }
        s.record_pod_owners(
            "c",
            10,
            &[
                ("apps".into(), "web-7d9f8b6c5-aaaaa".into(), "Deployment".into(), "web".into()),
                ("apps".into(), "web-api-6f7b9c8d4-ccccc".into(), "Deployment".into(), "web-api".into()),
            ],
        )
        .unwrap();

        let web = WorkloadRef { kind: "Deployment".into(), name: "web".into() };
        let q = LogQuery { namespace: Some("apps".into()), workload: Some(web.clone()), ..Default::default() };
        let pods: Vec<String> = s.query_logs("c", &q).unwrap().into_iter().map(|r| r.pod).collect();
        assert_eq!(pods, vec!["web-5c8d7f9b4-bbbbb", "web-7d9f8b6c5-aaaaa"]);
        let hist = s.log_histogram("c", &q, 60_000).unwrap();
        assert_eq!(hist.iter().map(|b| b.info + b.error).sum::<i64>(), 2);

        let no_ns = LogQuery { workload: Some(web.clone()), ..Default::default() };
        assert!(s.query_logs("c", &no_ns).is_err(), "workload filter needs a namespace");

        let sources = s.log_sources("c", Some("apps"), Some(&web)).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].pod, "web-5c8d7f9b4-bbbbb", "newest first");
        assert_eq!((sources[0].errors, sources[0].owner_kind.as_deref()), (1, None));
        assert_eq!(sources[1].owner_name.as_deref(), Some("web"));
        assert_eq!(s.log_sources("c", None, None).unwrap().len(), 4);

        let mut out = Vec::new();
        assert_eq!(s.write_workload_logs("c", "apps", &web, &mut out).unwrap(), 2);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("1970-01-01T00:00:01Z INFO    web-7d9f8b6c5-aaaaa/app hello"), "{}", lines[0]);
        assert!(lines[1].contains(" ERROR   web-5c8d7f9b4-bbbbb/app "));
    }

    #[test]
    fn fts_query_is_injection_safe() {
        let s = Store::open_in_memory().unwrap();
        // Unbalanced quotes / FTS operators must not error.
        s.query_logs("c", &LogQuery { search: Some(r#"foo" OR "bar NEAR("#.into()), ..Default::default() })
            .unwrap();
    }

    #[test]
    fn issue_lifecycle() {
        let s = Store::open_in_memory().unwrap();
        let issue = Issue {
            key: "k".into(),
            severity: Severity::Critical,
            category: "pod".into(),
            rule: "crashloopbackoff".into(),
            kind: "Pod".into(),
            namespace: Some("default".into()),
            name: "api".into(),
            title: "t".into(),
            detail: "d".into(),
            hint: None,
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        };
        let mut v = vec![issue.clone()];
        s.sync_issues("c", 100, &mut v).unwrap();
        assert_eq!(v[0].first_seen_ms, Some(100));
        let mut v = vec![issue];
        s.sync_issues("c", 200, &mut v).unwrap();
        assert_eq!(v[0].first_seen_ms, Some(100), "keeps original first-seen");
        s.sync_issues("c", 300, &mut []).unwrap();
        let h = s.issue_history("c", 0, 10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].resolved_ms, Some(300));
        assert_eq!(h[0].last_seen_ms, 200);
    }

    #[test]
    fn samples_bucket() {
        let s = Store::open_in_memory().unwrap();
        s.record_samples("c", 1000, &[("n1".into(), 1.0, 100.0)], &[]).unwrap();
        s.record_samples("c", 1500, &[("n1".into(), 3.0, 300.0)], &[]).unwrap();
        let h = s.node_history("c", "n1", 0, 60_000).unwrap();
        assert_eq!(h, vec![Sample { ts_ms: 0, cpu: 2.0, mem: 200.0 }]);
    }

    #[test]
    fn patterns_collapse_digits() {
        assert_eq!(normalize_pattern("timeout after 3012ms (try 2)"), "timeout after #ms (try #)");
    }

    #[test]
    fn unreadable_settings_are_kept_aside() {
        let s = Store::open_in_memory().unwrap();
        let bad = r#"{"connections":"not a list"}"#;
        s.conn.lock().unwrap().execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", params![SETTINGS_KEY, bad]).unwrap();
        assert!(s.load_settings().is_err());
        // Saving defaults over the row doesn't lose what was there.
        s.save_settings(&Settings::default()).unwrap();
        let kept: String = s
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = ?1", [SETTINGS_UNREADABLE_KEY], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, bad);
        assert_eq!(s.load_settings().unwrap(), Settings::default());
    }

    #[test]
    fn log_pages_follow_time_not_insert_order() {
        let s = Store::open_in_memory().unwrap();
        let cur = LogCursor { last_ts_ns: 0, restart_count: 0 };
        // "a" is pulled first with recent lines; "b" shows up later with older ones.
        s.append_logs("c", "u-a", "app", cur, &[pod_log("a", 500, Level::Info, "a1"), pod_log("a", 600, Level::Info, "a2")], 0).unwrap();
        s.append_logs("c", "u-b", "app", cur, &[pod_log("b", 100, Level::Info, "b1"), pod_log("b", 550, Level::Info, "b2")], 0).unwrap();
        let page = |before_id| {
            s.query_logs("c", &LogQuery { limit: Some(2), before_id, ..Default::default() }).unwrap()
        };
        let first = page(None);
        assert_eq!(first.iter().map(|r| r.message.as_str()).collect::<Vec<_>>(), ["a2", "b2"]);
        let second = page(Some(first[1].id));
        assert_eq!(second.iter().map(|r| r.message.as_str()).collect::<Vec<_>>(), ["a1", "b1"]);
        assert!(page(Some(second[1].id)).is_empty());
    }

    #[test]
    fn install_id_is_created_once() {
        let s = Store::open_in_memory().unwrap();
        let id = s.install_id().unwrap();
        assert_eq!(id.len(), 16);
        assert_eq!(s.install_id().unwrap(), id);
    }

    fn with_password(pw: &str) -> Settings {
        let json = format!(
            r#"{{"connections":[{{"id":"a","name":"a","connection":{{"mode":"ssh","host":"h","username":"u","auth":{{"kind":"password","password":"{pw}"}},"sudoPassword":"{pw}-sudo"}}}}]}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    fn raw_settings(s: &Store) -> String {
        s.conn.lock().unwrap().query_row("SELECT value FROM settings WHERE key = ?1", [SETTINGS_KEY], |r| r.get(0)).unwrap()
    }

    #[test]
    fn credentials_round_trip_and_are_encrypted_where_possible() {
        let s = Store::open_in_memory().unwrap();
        let mut st = with_password("hunter2");
        st.normalize();
        s.save_settings(&st).unwrap();
        assert_eq!(s.load_settings().unwrap(), st);
        assert_eq!(raw_settings(&s).contains("hunter2"), !portside_secrets::AVAILABLE);
    }

    #[test]
    fn plaintext_credentials_are_encrypted_on_load() {
        let s = Store::open_in_memory().unwrap();
        let old = serde_json::to_string(&with_password("hunter2")).unwrap();
        s.conn.lock().unwrap().execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", params![SETTINGS_KEY, old]).unwrap();
        let mut want = with_password("hunter2");
        want.normalize();
        assert_eq!(s.load_settings().unwrap(), want);
        assert_eq!(raw_settings(&s).contains("hunter2"), !portside_secrets::AVAILABLE);
    }

    #[test]
    fn undecryptable_credentials_are_dropped() {
        let s = Store::open_in_memory().unwrap();
        // As if the database came from another Windows account.
        let foreign = serde_json::to_string(&with_password("dpapi:00ff")).unwrap().replace("dpapi:00ff-sudo", "dpapi:abcd");
        s.conn.lock().unwrap().execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", params![SETTINGS_KEY, foreign]).unwrap();
        let loaded = s.load_settings().unwrap();
        let portside_core::Connection::Ssh(c) = &loaded.connections[0].connection else { panic!() };
        assert_eq!(c.auth, portside_core::SshAuth::Password { password: String::new() });
        assert_eq!(c.sudo_password.as_deref().unwrap_or_default(), "");
    }

    #[test]
    fn settings_default_when_empty() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.load_settings().unwrap(), Settings::default());
        let mut st = Settings::default();
        st.retention_days = 3;
        s.save_settings(&st).unwrap();
        assert_eq!(s.load_settings().unwrap().retention_days, 3);
    }
}
