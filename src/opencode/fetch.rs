//! "Fetch" OpenCode Go usage — a read-only aggregation over the opencode
//! CLI's local SQLite DB rather than an HTTP call. The cache/lock/fallback
//! skeleton matches the HTTP vendors so the widget shell treats every vendor
//! identically; the 60s TTL amortizes Waybar ticks across monitors.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, acquire_lock, home_dir};
use crate::error::{AppError, Result};
use crate::usage::OpencodeSnapshot;

use super::types::{Limits, WindowTotals, parse_cache, snap_to_json};

const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: OpencodeSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// Default opencode DB location — `~/.local/share/opencode/opencode.db` on
/// both Linux and macOS (opencode hardcodes `~/.local/share`, it does not use
/// the platform data dir).
pub fn default_db_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db"))
}

pub async fn fetch_snapshot(
    db_path: &Path,
    provider: &str,
    limits: &Limits,
    cache: &Cache,
    cache_ttl: Duration,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse_cache(bytes, cache, false));
    }

    match query_live(db_path, provider, now) {
        Ok(totals) => {
            let snap = totals.into_snapshot(provider, limits);
            let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap_or_default();
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        // SQLITE_BUSY/LOCKED while opencode holds the write lock — retry next
        // tick, serve cache silently (the local analog of a network blip).
        Err(e) if e.is_transient() => fallback_silent(cache),
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, e)
        }
    }
}

fn fallback_silent(cache: &Cache) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Transport(
            "opencode: no cache and database is busy".into(),
        ));
    };
    Ok(reuse_cache(bytes, cache, true))
}

/// Serve stale cache annotated with the error; with no cache at all, surface
/// the original error (it carries the "install opencode / set db_path" hint).
fn fallback_with_error(cache: &Cache, orig: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(orig);
    };
    let mut outcome = reuse_cache(bytes, cache, true);
    outcome.last_error = Some((0, orig.to_string()));
    Ok(outcome)
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> FetchOutcome {
    let snap = parse_cache(&bytes).unwrap_or_default();
    FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

const MS_5H: i64 = 5 * 3600 * 1000;
const MS_WEEK: i64 = 7 * 24 * 3600 * 1000;
const MS_MONTH: i64 = 30 * 24 * 3600 * 1000;

/// Aggregate spend/tokens for the given provider over the three rolling
/// windows, in one scan bounded to the last 30 days. `json_valid` guards
/// against malformed rows (json_extract would otherwise error the query).
fn query_live(db_path: &Path, provider: &str, now: DateTime<Utc>) -> Result<WindowTotals> {
    if !db_path.exists() {
        return Err(AppError::Other(format!(
            "opencode: database not found at {} — is the opencode CLI installed? \
             Otherwise set `db_path` under [opencode] in {}.",
            db_path.display(),
            crate::config::config_path_hint()
        )));
    }

    // Read-only open: never create, never write. The opencode CLI keeps the
    // DB in WAL mode; a read-only connection with a short busy timeout
    // coexists fine with a live writer. (`immutable=1` would be wrong here —
    // it makes SQLite ignore the WAL and the locking protocol entirely.)
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(map_sqlite_err)?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(map_sqlite_err)?;
    let _ = conn.pragma_update(None, "query_only", "ON");

    let now_ms = now.timestamp_millis();
    let (t5, tw, tm) = (now_ms - MS_5H, now_ms - MS_WEEK, now_ms - MS_MONTH);

    let (spent_5h, spent_week, spent_month, tokens_month) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN time_created >= ?1
                              THEN json_extract(data,'$.cost') END), 0.0),
                COALESCE(SUM(CASE WHEN time_created >= ?2
                              THEN json_extract(data,'$.cost') END), 0.0),
                COALESCE(SUM(json_extract(data,'$.cost')), 0.0),
                COALESCE(SUM(json_extract(data,'$.tokens.total')), 0.0)
             FROM message
             WHERE time_created >= ?3
               AND json_valid(data)
               AND json_extract(data,'$.providerID') = ?4",
            rusqlite::params![t5, tw, tm, provider],
            |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            },
        )
        .map_err(map_sqlite_err)?;

    let top_model: Option<String> = {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT json_extract(data,'$.modelID')
             FROM message
             WHERE time_created >= ?1
               AND json_valid(data)
               AND json_extract(data,'$.providerID') = ?2
               AND json_extract(data,'$.modelID') IS NOT NULL
             GROUP BY 1 ORDER BY COUNT(*) DESC, 1 LIMIT 1",
            rusqlite::params![tm, provider],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_err)?
    };

    Ok(WindowTotals {
        spent_5h,
        spent_week,
        spent_month,
        tokens_month: tokens_month as i64,
        top_model,
    })
}

/// Busy/locked → transient (writer active, retry next tick); missing table or
/// column → schema drift (opencode changed its storage layout); rest → Other.
fn map_sqlite_err(e: rusqlite::Error) -> AppError {
    if let rusqlite::Error::SqliteFailure(err, _) = &e
        && matches!(
            err.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return AppError::Transport(format!("opencode db busy: {e}"));
    }
    let msg = e.to_string();
    if msg.contains("no such table") || msg.contains("no such column") {
        AppError::Schema(format!("opencode db layout drift: {msg}"))
    } else {
        AppError::Other(format!("opencode db: {msg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Frozen "now" so window membership is deterministic.
    fn now() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 7, 1, 12, 0, 0).unwrap()
    }

    fn limits() -> Limits {
        Limits {
            five_hour: 12.0,
            week: 30.0,
            month: 60.0,
        }
    }

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("opencode"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn msg_json(provider: &str, model: &str, cost: f64, tokens: i64) -> String {
        serde_json::json!({
            "role": "assistant",
            "providerID": provider,
            "modelID": model,
            "cost": cost,
            "tokens": {"total": tokens, "input": tokens, "output": 0, "reasoning": 0,
                        "cache": {"read": 0, "write": 0}},
        })
        .to_string()
    }

    /// Build a realistic opencode DB in `dir`: the `message` table with the
    /// columns the query relies on, in WAL mode like the real CLI's DB.
    fn fixture_db(dir: &Path, rows: &[(i64, String)]) -> PathBuf {
        let path = dir.join("opencode.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        for (i, (ts, data)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                 VALUES (?1, 'ses', ?2, ?2, ?3)",
                rusqlite::params![format!("msg_{i}"), ts, data],
            )
            .unwrap();
        }
        path
    }

    fn ms_ago(hours: i64) -> i64 {
        (now() - chrono::Duration::hours(hours)).timestamp_millis()
    }

    #[tokio::test]
    async fn sums_costs_per_rolling_window() {
        let td = TempDir::new().unwrap();
        let rows = vec![
            // In the 5h window (and therefore week + month too).
            (ms_ago(1), msg_json("opencode-go", "deepseek-v4-pro", 1.0, 100)),
            // Past 5h, inside the week.
            (ms_ago(6), msg_json("opencode-go", "deepseek-v4-pro", 2.0, 200)),
            (ms_ago(3 * 24), msg_json("opencode-go", "deepseek-v4-flash", 4.0, 400)),
            // Past the week, inside 30d.
            (ms_ago(10 * 24), msg_json("opencode-go", "deepseek-v4-pro", 8.0, 800)),
            // Outside the 30d window entirely.
            (ms_ago(40 * 24), msg_json("opencode-go", "deepseek-v4-pro", 16.0, 1600)),
        ];
        let db = fixture_db(td.path(), &rows);
        let (_ctd, cache) = cache_fixture();

        let out = fetch_snapshot(&db, "opencode-go", &limits(), &cache, Duration::ZERO, now())
            .await
            .unwrap();
        let s = &out.snapshot;
        assert!((s.spent_5h - 1.0).abs() < 1e-9, "5h: {}", s.spent_5h);
        assert!((s.spent_week - 7.0).abs() < 1e-9, "week: {}", s.spent_week);
        assert!(
            (s.spent_month - 15.0).abs() < 1e-9,
            "month: {}",
            s.spent_month
        );
        assert_eq!(s.tokens_month, 1500);
        assert_eq!(s.top_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(s.limit_5h, 12.0);
        assert!(!out.stale);
        assert!(out.last_error.is_none());
    }

    #[tokio::test]
    async fn filters_rows_to_the_requested_provider() {
        let td = TempDir::new().unwrap();
        let rows = vec![
            (ms_ago(1), msg_json("opencode-go", "deepseek-v4-pro", 1.0, 100)),
            (ms_ago(1), msg_json("opencode", "big-pickle", 2.0, 200)),
            (ms_ago(1), msg_json("openrouter", "gpt-5", 4.0, 400)),
        ];
        let db = fixture_db(td.path(), &rows);
        let (_ctd, cache) = cache_fixture();

        let out = fetch_snapshot(&db, "opencode-go", &limits(), &cache, Duration::ZERO, now())
            .await
            .unwrap();
        assert!((out.snapshot.spent_5h - 1.0).abs() < 1e-9);
        assert_eq!(out.snapshot.tokens_month, 100);
    }

    #[tokio::test]
    async fn malformed_json_and_costless_rows_are_ignored() {
        let td = TempDir::new().unwrap();
        let rows = vec![
            (ms_ago(1), msg_json("opencode-go", "deepseek-v4-pro", 1.0, 100)),
            (ms_ago(1), "{not valid json".to_string()),
            // user message: no providerID / cost / tokens.
            (ms_ago(1), r#"{"role":"user","time":{}}"#.to_string()),
        ];
        let db = fixture_db(td.path(), &rows);
        let (_ctd, cache) = cache_fixture();

        let out = fetch_snapshot(&db, "opencode-go", &limits(), &cache, Duration::ZERO, now())
            .await
            .unwrap();
        assert!((out.snapshot.spent_5h - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn empty_db_yields_zeroed_snapshot() {
        let td = TempDir::new().unwrap();
        let db = fixture_db(td.path(), &[]);
        let (_ctd, cache) = cache_fixture();

        let out = fetch_snapshot(&db, "opencode-go", &limits(), &cache, Duration::ZERO, now())
            .await
            .unwrap();
        assert_eq!(out.snapshot.spent_month, 0.0);
        assert_eq!(out.snapshot.tokens_month, 0);
        assert_eq!(out.snapshot.top_model, None);
        // Limits still ride along so the renderer can show "$0 of $12".
        assert_eq!(out.snapshot.limit_5h, 12.0);
    }

    #[tokio::test]
    async fn missing_db_without_cache_errors_with_install_hint() {
        let td = TempDir::new().unwrap();
        let (_ctd, cache) = cache_fixture();
        let missing = td.path().join("nope").join("opencode.db");

        let err = fetch_snapshot(
            &missing,
            "opencode-go",
            &limits(),
            &cache,
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("opencode"), "message names the CLI: {msg}");
        assert!(msg.contains("db_path"), "message points at config: {msg}");
    }

    #[tokio::test]
    async fn schema_drift_falls_back_to_cache_as_stale() {
        let td = TempDir::new().unwrap();
        // A DB without the `message` table simulates an opencode schema change.
        let db = td.path().join("opencode.db");
        rusqlite::Connection::open(&db).unwrap();
        let (_ctd, cache) = cache_fixture();
        let seed = snap_to_json(
            &WindowTotals {
                spent_5h: 0.5,
                spent_week: 3.0,
                spent_month: 12.0,
                tokens_month: 42,
                top_model: None,
            }
            .into_snapshot("opencode-go", &limits()),
        );
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let out = fetch_snapshot(&db, "opencode-go", &limits(), &cache, Duration::ZERO, now())
            .await
            .unwrap();
        assert!(out.stale);
        assert!((out.snapshot.spent_week - 3.0).abs() < 1e-9);
        assert!(out.last_error.is_some());
    }

    #[tokio::test]
    async fn fresh_cache_skips_the_db_entirely() {
        let td = TempDir::new().unwrap();
        let (_ctd, cache) = cache_fixture();
        let seed = snap_to_json(
            &WindowTotals {
                spent_5h: 2.0,
                ..Default::default()
            }
            .into_snapshot("opencode-go", &limits()),
        );
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        // db_path doesn't even exist — the fresh cache must short-circuit.
        let missing = td.path().join("nope.db");
        let out = fetch_snapshot(
            &missing,
            "opencode-go",
            &limits(),
            &cache,
            Duration::from_secs(3600),
            now(),
        )
        .await
        .unwrap();
        assert!(!out.stale);
        assert!((out.snapshot.spent_5h - 2.0).abs() < 1e-9);
    }
}
