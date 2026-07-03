//! End-to-end integration test for the OpenCode Go vendor.
//!
//! Builds a temp SQLite DB shaped like the opencode CLI's `message` table,
//! drives the full `fetch_snapshot` + `opencode::vendor::render` pipeline,
//! and asserts the resulting Waybar JSON via `insta` snapshots. The fixture
//! schema doubles as a canary for opencode storage-layout drift.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ai_usagebar::cache::Cache;
use ai_usagebar::format::updated_at_hm;
use ai_usagebar::opencode::{self, types::Limits};
use ai_usagebar::theme::Theme;
use ai_usagebar::vendor::{RenderOpts, VendorOutcome};
use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap()
}

fn limits() -> Limits {
    Limits {
        five_hour: 12.0,
        week: 30.0,
        month: 60.0,
    }
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

fn fixture_db(dir: &Path) -> PathBuf {
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
    let rows = [
        (1_i64, msg_json("opencode-go", "deepseek-v4-pro", 1.29, 44_455)),
        (4, msg_json("opencode-go", "deepseek-v4-pro", 2.87, 120_000)),
        (26, msg_json("opencode-go", "deepseek-v4-flash", 4.34, 900_000)),
        (240, msg_json("opencode-go", "deepseek-v4-pro", 36.50, 3_500_000)),
        // Other providers must not count toward the Go windows.
        (1, msg_json("opencode", "big-pickle", 9.99, 1_000)),
    ];
    for (i, (hours_ago, data)) in rows.iter().enumerate() {
        let ts = (now() - chrono::Duration::hours(*hours_ago)).timestamp_millis();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, 'ses', ?2, ?2, ?3)",
            rusqlite::params![format!("msg_{i}"), ts, data],
        )
        .unwrap();
    }
    path
}

fn normalize_updated_time(
    tooltip: &str,
    now: DateTime<Utc>,
    cache_age: Option<Duration>,
) -> String {
    let updated = updated_at_hm(now, cache_age);
    let expected = format!("Updated {updated}");
    assert!(
        tooltip.contains(&expected),
        "tooltip did not contain {expected:?}"
    );
    tooltip.replace(&expected, "Updated HH:MM")
}

fn opts() -> RenderOpts {
    RenderOpts {
        format: None,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
    }
}

#[tokio::test]
async fn full_db_renders_expected_waybar_json() {
    let td = TempDir::new().unwrap();
    let db = fixture_db(td.path());
    let cache = Cache::at(td.path().join("cache").join("opencode"));

    let outcome = opencode::fetch_snapshot(
        &db,
        "opencode-go",
        &limits(),
        &cache,
        Duration::from_secs(0),
        now(),
    )
    .await
    .unwrap();

    // 5h: 1.29+2.87 = 4.16 (35%); week: +4.34 = 8.50 (28%); month: +36.50 =
    // 45.00 (75%) → worst window High.
    let snap = outcome.snapshot.clone();
    let cache_age = outcome.cache_age;
    let vendor_outcome: VendorOutcome = outcome.into();
    let theme = Theme::default(); // One Dark — stable across machines
    let out = opencode::vendor::render(&vendor_outcome, &snap, &theme, &opts(), now());

    insta::assert_snapshot!("opencode_full_bar_text", &out.text);
    insta::assert_snapshot!(
        "opencode_full_tooltip",
        normalize_updated_time(&out.tooltip, now(), cache_age)
    );
    assert_eq!(format!("{:?}", out.class), "High");
}

#[tokio::test]
async fn missing_db_with_no_cache_is_a_credible_widget_error() {
    let td = TempDir::new().unwrap();
    let cache = Cache::at(td.path().join("cache").join("opencode"));
    let err = opencode::fetch_snapshot(
        &td.path().join("missing").join("opencode.db"),
        "opencode-go",
        &limits(),
        &cache,
        Duration::from_secs(0),
        now(),
    )
    .await
    .unwrap_err();
    // The widget shell turns this into the ⚠ fallback; the message must be
    // actionable for someone without the opencode CLI installed.
    let msg = err.to_string();
    assert!(msg.contains("opencode"));
    assert!(msg.contains("db_path"));
}
