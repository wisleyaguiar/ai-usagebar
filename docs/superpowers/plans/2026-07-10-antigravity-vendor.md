# Antigravity Vendor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `antigravity` vendor that shows Google Antigravity IDE plan quota (4 buckets: Gemini 5h/weekly + Claude+GPT 5h/weekly) in the Waybar widget, TUI, and macOS menu bar, by probing the IDE's local language server.

**Architecture:** Local-first vendor (like `opencode`, but HTTP instead of SQLite): discover the Antigravity language-server process via `ps`, its listening ports via `lsof`, then POST Connect-RPC JSON to `https://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary` with the `--csrf_token` scraped from the process command line. Normalize into `AntigravitySnapshot` (4 optional `UsageWindow`s), cache the normalized JSON with the existing atomic/flock `Cache`.

**Tech Stack:** Rust (edition 2024), reqwest 0.12 (rustls), tokio (process feature already enabled), serde, chrono, mockito + insta for tests. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-10-antigravity-vendor-design.md`

## Global Constraints

- **Widget always exits 0** — errors become the ⚠ fallback JSON (handled by the widget shell; vendor code returns `Result`).
- **Tests are hermetic** — never read real `$HOME`/`$XDG`, never depend on a real process list or real network. Use `Cache::at`, injected `&str` inputs for parsers, injected endpoint lists + mockito for HTTP.
- **`danger_accept_invalid_certs(true)` only on the dedicated loopback client** (`loopback_client()`), never on the shared `http_client()`.
- Gate before finishing: `cargo test` (all pass), `cargo clippy --all-targets -- -D warnings` (clean), `cargo machete` (clean).
- Commit messages: Conventional Commits, end body with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Rust naming: module is `src/antigravity/`, slug/config section/cache dir all `"antigravity"`.
- All code comments in English, matching repo style.

---

### Task 1: `AntigravitySnapshot` in usage.rs + TUI panel sections

**Files:**
- Modify: `src/usage.rs` (add struct after `ZaiSnapshot` around line 230; add enum variant at line ~183)
- Modify: `src/tui/panels.rs` (match arm at line ~85, new `antigravity_sections` fn after `opencode_sections` ~line 313)

**Interfaces:**
- Produces: `crate::usage::AntigravitySnapshot` with public fields `gemini_session`, `gemini_weekly`, `claude_gpt_session`, `claude_gpt_weekly` (all `Option<UsageWindow>`), method `max_pct() -> i32`, and `VendorSnapshot::Antigravity(AntigravitySnapshot)`. Implements `Debug, Clone, PartialEq, Eq, Default`.
- Consumes: existing `UsageWindow` (fields `utilization_pct: i32`, `resets_at: Option<DateTime<Utc>>`, `window_duration: chrono::Duration`).

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `src/usage.rs`:

```rust
    fn ag_w(pct: i32) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: None,
            window_duration: Duration::hours(5),
        }
    }

    #[test]
    fn antigravity_max_pct_picks_worst_bucket() {
        let snap = AntigravitySnapshot {
            gemini_session: Some(ag_w(10)),
            gemini_weekly: Some(ag_w(80)),
            claude_gpt_session: Some(ag_w(45)),
            claude_gpt_weekly: None,
        };
        assert_eq!(snap.max_pct(), 80);
    }

    #[test]
    fn antigravity_max_pct_empty_is_zero() {
        assert_eq!(AntigravitySnapshot::default().max_pct(), 0);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib usage::tests::antigravity -- --nocapture`
Expected: compile error — `AntigravitySnapshot` not found.

- [ ] **Step 3: Implement** — in `src/usage.rs`, after the `ZaiSnapshot` struct (~line 230):

```rust
/// Google Antigravity IDE — four quota buckets probed from the IDE's local
/// language server (`RetrieveUserQuotaSummary`). Two model groups × two
/// windows; every bucket is optional because the server may omit a group
/// (e.g. a plan without Claude/GPT access).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravitySnapshot {
    /// "Gemini Models" group, 5-hour window.
    pub gemini_session: Option<UsageWindow>,
    /// "Gemini Models" group, weekly window.
    pub gemini_weekly: Option<UsageWindow>,
    /// "Claude and GPT models" group, 5-hour window.
    pub claude_gpt_session: Option<UsageWindow>,
    /// "Claude and GPT models" group, weekly window.
    pub claude_gpt_weekly: Option<UsageWindow>,
}

impl AntigravitySnapshot {
    /// Worst-of percentage across all present buckets — drives severity.
    pub fn max_pct(&self) -> i32 {
        [
            &self.gemini_session,
            &self.gemini_weekly,
            &self.claude_gpt_session,
            &self.claude_gpt_weekly,
        ]
        .into_iter()
        .filter_map(|w| w.as_ref().map(|w| w.utilization_pct))
        .max()
        .unwrap_or(0)
    }
}
```

And add the variant to the `VendorSnapshot` enum (line ~183):

```rust
    Antigravity(AntigravitySnapshot),
```

- [ ] **Step 4: Fix the exhaustive match in `src/tui/panels.rs`** — add arm at line ~85:

```rust
                VendorSnapshot::Antigravity(s) => antigravity_sections(s, now, pace_tolerance),
```

and the sections fn after `opencode_sections` (~line 313):

```rust
fn antigravity_sections(
    s: &crate::usage::AntigravitySnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: "Antigravity".into(),
        right: None,
    }];
    if let Some(w) = &s.gemini_session {
        push_window(&mut v, "Gemini 5h", w, now, tol, true);
    }
    if let Some(w) = &s.gemini_weekly {
        push_window(&mut v, "Gemini weekly", w, now, tol, true);
    }
    if let Some(w) = &s.claude_gpt_session {
        push_window(&mut v, "Claude+GPT 5h", w, now, tol, true);
    }
    if let Some(w) = &s.claude_gpt_weekly {
        push_window(&mut v, "Claude+GPT weekly", w, now, tol, true);
    }
    if s.gemini_session.is_none()
        && s.gemini_weekly.is_none()
        && s.claude_gpt_session.is_none()
        && s.claude_gpt_weekly.is_none()
    {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "".into(),
            value: "no quota buckets reported".into(),
        });
    }
    v
}
```

- [ ] **Step 5: Add a panels test** — inside `mod tests` of `src/tui/panels.rs`, next to `opencode_sections_have_three_window_metrics_and_usage_block` (~line 707), reusing that module's existing `ready()`/`sections_for()`/`now()` helpers:

```rust
    #[test]
    fn antigravity_sections_render_all_four_buckets() {
        let w = |pct: i32| crate::usage::UsageWindow {
            utilization_pct: pct,
            resets_at: None,
            window_duration: chrono::Duration::hours(5),
        };
        let snap = crate::usage::AntigravitySnapshot {
            gemini_session: Some(w(63)),
            gemini_weekly: Some(w(28)),
            claude_gpt_session: Some(w(90)),
            claude_gpt_weekly: Some(w(45)),
        };
        let sections = sections_for(&ready(VendorSnapshot::Antigravity(snap)), now(), 5);
        let metrics = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metrics, 4);
    }
```

(If `ready()` / `sections_for()` have different signatures in that test module, mirror exactly how `opencode_sections_have_three_window_metrics_and_usage_block` builds its input.)

- [ ] **Step 6: Run tests**

Run: `cargo test --lib`
Expected: PASS (all existing + new).

- [ ] **Step 7: Commit**

```bash
git add src/usage.rs src/tui/panels.rs
git commit -m "feat(antigravity): AntigravitySnapshot + TUI panel sections"
```

---

### Task 2: `src/antigravity/types.rs` — wire parsing + snapshot mapping + cache round-trip

**Files:**
- Create: `src/antigravity/mod.rs`
- Create: `src/antigravity/types.rs`
- Modify: `src/lib.rs` (add `pub mod antigravity;` in alphabetical order, near line 20)

**Interfaces:**
- Consumes: `crate::usage::{AntigravitySnapshot, UsageWindow}` (Task 1).
- Produces:
  - `QuotaSummaryResponse` (serde `Deserialize`) with `into_snapshot(self) -> AntigravitySnapshot`
  - `UserStatusResponse` (legacy fallback) with `into_snapshot(self) -> AntigravitySnapshot`
  - `snap_to_json(&AntigravitySnapshot) -> serde_json::Value`
  - `parse_cache(&[u8]) -> Result<AntigravitySnapshot>`
  - `parse_reset_time(&serde_json::Value) -> Option<DateTime<Utc>>`

- [ ] **Step 1: Create module skeleton**

`src/antigravity/mod.rs`:

```rust
//! Google Antigravity vendor — a local-first vendor that probes the
//! Antigravity IDE's language server over loopback HTTPS (Connect-RPC) for
//! plan quota buckets. No credentials leave the machine; the CSRF token and
//! port come from the running process itself. When the IDE isn't running,
//! the cached snapshot is served (marked stale).

pub mod types;
```

`src/lib.rs`: add `pub mod antigravity;` (alphabetical, before `pub mod anthropic;` if that ordering is used, otherwise next to it).

- [ ] **Step 2: Write the failing tests** — bottom of `src/antigravity/types.rs` (create the file with just a `#[cfg(test)] mod tests` first if you want a strict red step, but writing types+tests together and asserting failures via `todo!()` bodies is acceptable here; the meaningful red is the fixture parse):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SUMMARY_FIXTURE: &str = r#"{
        "groups": [
            {"displayName": "Gemini Models", "buckets": [
                {"bucketId": "gemini-5h", "displayName": "5-hour limit",
                 "remaining": {"remainingFraction": 0.37},
                 "resetTime": "2026-07-01T14:30:00Z"},
                {"bucketId": "gemini-weekly", "displayName": "Weekly limit",
                 "remaining": {"remainingFraction": 0.72},
                 "resetTime": "2026-07-05T00:00:00Z"}
            ]},
            {"displayName": "Claude and GPT models", "buckets": [
                {"bucketId": "claude-gpt-5h", "displayName": "5-hour limit",
                 "remaining": {"remainingFraction": 0.10},
                 "resetTime": "2026-07-01T14:30:00Z"},
                {"bucketId": "claude-gpt-weekly", "displayName": "Weekly limit",
                 "remaining": {"remainingFraction": 0.55}}
            ]}
        ]
    }"#;

    #[test]
    fn summary_fixture_maps_to_four_buckets() {
        let resp: QuotaSummaryResponse = serde_json::from_str(SUMMARY_FIXTURE).unwrap();
        let snap = resp.into_snapshot();
        // used = round((1 - remainingFraction) * 100)
        assert_eq!(snap.gemini_session.as_ref().unwrap().utilization_pct, 63);
        assert_eq!(snap.gemini_weekly.as_ref().unwrap().utilization_pct, 28);
        assert_eq!(snap.claude_gpt_session.as_ref().unwrap().utilization_pct, 90);
        assert_eq!(snap.claude_gpt_weekly.as_ref().unwrap().utilization_pct, 45);
        // Reset times parse; the weekly claude bucket has none.
        assert_eq!(
            snap.gemini_session.as_ref().unwrap().resets_at,
            chrono::Utc.with_ymd_and_hms(2026, 7, 1, 14, 30, 0).single()
        );
        assert_eq!(snap.claude_gpt_weekly.as_ref().unwrap().resets_at, None);
        // Window durations drive pacing math.
        assert_eq!(
            snap.gemini_session.as_ref().unwrap().window_duration,
            chrono::Duration::hours(5)
        );
        assert_eq!(
            snap.gemini_weekly.as_ref().unwrap().window_duration,
            chrono::Duration::days(7)
        );
    }

    #[test]
    fn unknown_group_and_missing_fraction_are_skipped() {
        let json = r#"{"groups": [
            {"displayName": "Mystery Models", "buckets": [
                {"bucketId": "m-5h", "remaining": {"remainingFraction": 0.5}}
            ]},
            {"displayName": "Gemini Models", "buckets": [
                {"bucketId": "gemini-5h", "displayName": "5-hour limit",
                 "remaining": {}}
            ]}
        ]}"#;
        let resp: QuotaSummaryResponse = serde_json::from_str(json).unwrap();
        let snap = resp.into_snapshot();
        assert_eq!(snap, crate::usage::AntigravitySnapshot::default());
    }

    #[test]
    fn empty_response_parses_to_default() {
        let resp: QuotaSummaryResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(resp.into_snapshot(), crate::usage::AntigravitySnapshot::default());
    }

    #[test]
    fn reset_time_parses_iso8601_and_epoch() {
        let iso = serde_json::json!("2026-07-01T14:30:00Z");
        let epoch = serde_json::json!(1_782_951_000);
        let epoch_str = serde_json::json!("1782951000");
        assert!(parse_reset_time(&iso).is_some());
        assert_eq!(
            parse_reset_time(&epoch),
            chrono::Utc.timestamp_opt(1_782_951_000, 0).single()
        );
        assert_eq!(parse_reset_time(&epoch_str), parse_reset_time(&epoch));
        assert_eq!(parse_reset_time(&serde_json::json!(null)), None);
    }

    #[test]
    fn user_status_fallback_maps_worst_model_per_group_as_session() {
        let json = r#"{"userStatus": {"cascadeModelConfigData": {"clientModelConfigs": [
            {"label": "Gemini 3 Pro", "quotaInfo": {"remainingFraction": 0.4}},
            {"label": "Gemini 3 Flash", "quotaInfo": {"remainingFraction": 0.9}},
            {"label": "Claude Sonnet 4.5", "quotaInfo": {"remainingFraction": 0.2,
                "resetTime": "2026-07-01T14:30:00Z"}}
        ]}}}"#;
        let resp: UserStatusResponse = serde_json::from_str(json).unwrap();
        let snap = resp.into_snapshot();
        // Worst (lowest remaining) per group wins: gemini 0.4 → 60%.
        assert_eq!(snap.gemini_session.as_ref().unwrap().utilization_pct, 60);
        assert_eq!(snap.claude_gpt_session.as_ref().unwrap().utilization_pct, 80);
        assert!(snap.claude_gpt_session.as_ref().unwrap().resets_at.is_some());
        assert_eq!(snap.gemini_weekly, None);
        assert_eq!(snap.claude_gpt_weekly, None);
    }

    #[test]
    fn cache_round_trip_preserves_snapshot() {
        let resp: QuotaSummaryResponse = serde_json::from_str(SUMMARY_FIXTURE).unwrap();
        let snap = resp.into_snapshot();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        let parsed = parse_cache(&bytes).unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn parse_cache_defaults_missing_fields() {
        let parsed = parse_cache(b"{}").unwrap();
        assert_eq!(parsed, crate::usage::AntigravitySnapshot::default());
    }
}
```

- [ ] **Step 3: Run to verify red**

Run: `cargo test --lib antigravity`
Expected: compile error (types don't exist yet).

- [ ] **Step 4: Implement `src/antigravity/types.rs`**

```rust
//! Wire types for the language server's Connect-RPC JSON responses
//! (`RetrieveUserQuotaSummary` preferred, `GetUserStatus` legacy fallback)
//! plus the normalized cache serialization.
//!
//! The endpoints are undocumented; field names follow what the Antigravity
//! IDE itself renders in Settings → Models (see the design spec). Every
//! field is optional/defaulted — schema drift must degrade, not panic.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::error::Result;
use crate::usage::{AntigravitySnapshot, UsageWindow};

/// `RetrieveUserQuotaSummary` response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSummaryResponse {
    #[serde(default)]
    pub groups: Vec<QuotaGroup>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaGroup {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    #[serde(default)]
    pub bucket_id: String,
    #[serde(default)]
    pub display_name: String,
    pub remaining: Option<RemainingQuota>,
    /// ISO-8601 string or epoch seconds — shape varies, parsed leniently.
    pub reset_time: Option<serde_json::Value>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemainingQuota {
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<serde_json::Value>,
}

/// `GetUserStatus` legacy response — per-model quota, no group/window
/// structure. Mapped to session buckets only (worst model per group).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStatusResponse {
    pub user_status: Option<UserStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStatus {
    pub cascade_model_config_data: Option<CascadeModelConfigData>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CascadeModelConfigData {
    #[serde(default)]
    pub client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientModelConfig {
    #[serde(default)]
    pub label: String,
    pub quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaInfo {
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelGroup {
    Gemini,
    ClaudeGpt,
}

fn classify_group(name: &str) -> Option<ModelGroup> {
    let l = name.to_lowercase();
    if l.contains("gemini") {
        Some(ModelGroup::Gemini)
    } else if l.contains("claude") || l.contains("gpt") {
        Some(ModelGroup::ClaudeGpt)
    } else {
        None
    }
}

/// Weekly vs 5h window, inferred from bucket id + display name.
fn is_weekly(bucket: &QuotaBucket) -> bool {
    let hay = format!("{} {}", bucket.bucket_id, bucket.display_name).to_lowercase();
    hay.contains("week") || hay.contains("7d")
}

/// `remainingFraction` is "how much is left"; the bar shows "how much used".
fn used_pct(remaining_fraction: f64) -> i32 {
    (((1.0 - remaining_fraction) * 100.0).round()).clamp(0.0, 100.0) as i32
}

/// Lenient reset-time parse: RFC-3339 string, epoch-seconds number, or
/// epoch-seconds-as-string. Anything else → None.
pub fn parse_reset_time(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    match v {
        serde_json::Value::String(s) => {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&Utc));
            }
            s.parse::<i64>().ok().and_then(|secs| Utc.timestamp_opt(secs, 0).single())
        }
        serde_json::Value::Number(n) => {
            n.as_i64().and_then(|secs| Utc.timestamp_opt(secs, 0).single())
        }
        _ => None,
    }
}

fn window(pct: i32, resets_at: Option<DateTime<Utc>>, weekly: bool) -> UsageWindow {
    UsageWindow {
        utilization_pct: pct,
        resets_at,
        window_duration: if weekly {
            chrono::Duration::days(7)
        } else {
            chrono::Duration::hours(5)
        },
    }
}

impl QuotaSummaryResponse {
    pub fn into_snapshot(self) -> AntigravitySnapshot {
        let mut snap = AntigravitySnapshot::default();
        for group in &self.groups {
            let Some(g) = classify_group(&group.display_name) else {
                continue;
            };
            for bucket in &group.buckets {
                let Some(frac) = bucket.remaining.as_ref().and_then(|r| r.remaining_fraction)
                else {
                    continue;
                };
                let reset = bucket
                    .reset_time
                    .as_ref()
                    .or(bucket.remaining.as_ref().and_then(|r| r.reset_time.as_ref()))
                    .and_then(parse_reset_time);
                let weekly = is_weekly(bucket);
                let w = window(used_pct(frac), reset, weekly);
                let slot = match (g, weekly) {
                    (ModelGroup::Gemini, false) => &mut snap.gemini_session,
                    (ModelGroup::Gemini, true) => &mut snap.gemini_weekly,
                    (ModelGroup::ClaudeGpt, false) => &mut snap.claude_gpt_session,
                    (ModelGroup::ClaudeGpt, true) => &mut snap.claude_gpt_weekly,
                };
                // First bucket per slot wins — duplicates would mean the
                // grouping heuristic broke; keep the first, don't overwrite.
                if slot.is_none() {
                    *slot = Some(w);
                }
            }
        }
        snap
    }
}

impl UserStatusResponse {
    /// Legacy fallback: per-model quota with no window info. Take the worst
    /// (lowest remaining) model per group and expose it as the 5h session
    /// bucket; weekly stays None.
    pub fn into_snapshot(self) -> AntigravitySnapshot {
        let mut snap = AntigravitySnapshot::default();
        let configs = self
            .user_status
            .and_then(|u| u.cascade_model_config_data)
            .map(|c| c.client_model_configs)
            .unwrap_or_default();
        let mut worst: [Option<(f64, Option<DateTime<Utc>>)>; 2] = [None, None];
        for cfg in &configs {
            let Some(g) = classify_group(&cfg.label) else {
                continue;
            };
            let Some(frac) = cfg.quota_info.as_ref().and_then(|q| q.remaining_fraction) else {
                continue;
            };
            let reset = cfg
                .quota_info
                .as_ref()
                .and_then(|q| q.reset_time.as_ref())
                .and_then(parse_reset_time);
            let idx = match g {
                ModelGroup::Gemini => 0,
                ModelGroup::ClaudeGpt => 1,
            };
            if worst[idx].is_none_or(|(f, _)| frac < f) {
                worst[idx] = Some((frac, reset));
            }
        }
        if let Some((f, r)) = worst[0] {
            snap.gemini_session = Some(window(used_pct(f), r, false));
        }
        if let Some((f, r)) = worst[1] {
            snap.claude_gpt_session = Some(window(used_pct(f), r, false));
        }
        snap
    }
}

/// Serialize a snapshot for the per-vendor cache file (normalized — the raw
/// wire bytes are NOT cached, so both endpoints share one cache shape).
pub fn snap_to_json(snap: &AntigravitySnapshot) -> serde_json::Value {
    fn win(w: &Option<UsageWindow>) -> serde_json::Value {
        match w {
            None => serde_json::Value::Null,
            Some(w) => serde_json::json!({
                "pct": w.utilization_pct,
                "resets_at": w.resets_at.map(|t| t.to_rfc3339()),
                "weekly": w.window_duration == chrono::Duration::days(7),
            }),
        }
    }
    serde_json::json!({
        "gemini_session": win(&snap.gemini_session),
        "gemini_weekly": win(&snap.gemini_weekly),
        "claude_gpt_session": win(&snap.claude_gpt_session),
        "claude_gpt_weekly": win(&snap.claude_gpt_weekly),
    })
}

/// Parse a cached payload back. Missing fields default — same forgiving
/// posture as the other vendors' `parse_cache`.
pub fn parse_cache(bytes: &[u8]) -> Result<AntigravitySnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    fn win(v: &serde_json::Value) -> Option<UsageWindow> {
        if !v.is_object() {
            return None;
        }
        let weekly = v["weekly"].as_bool().unwrap_or(false);
        Some(UsageWindow {
            utilization_pct: v["pct"].as_i64().unwrap_or(0) as i32,
            resets_at: v["resets_at"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc)),
            window_duration: if weekly {
                chrono::Duration::days(7)
            } else {
                chrono::Duration::hours(5)
            },
        })
    }
    Ok(AntigravitySnapshot {
        gemini_session: win(&v["gemini_session"]),
        gemini_weekly: win(&v["gemini_weekly"]),
        claude_gpt_session: win(&v["claude_gpt_session"]),
        claude_gpt_weekly: win(&v["claude_gpt_weekly"]),
    })
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib antigravity`
Expected: PASS (all 7 tests).

- [ ] **Step 6: Commit**

```bash
git add src/antigravity/ src/lib.rs
git commit -m "feat(antigravity): wire types, snapshot mapping, cache round-trip"
```

---

### Task 3: `src/antigravity/fetch.rs` — process/port discovery parsers (pure)

**Files:**
- Create: `src/antigravity/fetch.rs`
- Modify: `src/antigravity/mod.rs` (add `pub mod fetch;` and re-exports)

**Interfaces:**
- Produces (all pure, string-in/data-out — the hermetic test seam):
  - `pub struct ProcessCandidate { pub pid: u32, pub csrf_token: Option<String>, pub extension_port: Option<u16>, pub extension_csrf_token: Option<String> }`
  - `pub fn candidates_from_ps_output(out: &str) -> Vec<ProcessCandidate>`
  - `pub fn ports_from_lsof_output(out: &str) -> Vec<u16>`
- Consumed by: Task 4 (`discover_endpoints`) in this same file.

- [ ] **Step 1: Write the failing tests** — create `src/antigravity/fetch.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Realistic-shaped ps lines (paths shortened). The IDE language server
    // carries --csrf_token; the CLI (agy) one doesn't need it.
    const PS_FIXTURE: &str = "\
  312 /usr/lib/systemd/systemd --user
16970 /Applications/Antigravity IDE.app/Contents/Frameworks/Electron Framework.framework/Helpers/chrome_crashpad_handler --no-rate-limit
40001 /Applications/Antigravity IDE.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --app_data_dir antigravity --csrf_token 1e2d3c4b-aaaa-bbbb-cccc-000000000001 --extension_server_port 42100 --extension_server_csrf_token 9f8e7d6c-dddd-eeee-ffff-000000000002
40002 /Users/me/.codeium/bin/language_server_macos_arm --app_data_dir codeium
40003 /Users/me/.antigravity/antigravity/bin/agy serve
40004 /opt/homebrew/bin/antigravity-cli --serve
";

    #[test]
    fn ps_parse_finds_ide_server_with_tokens() {
        let cands = candidates_from_ps_output(PS_FIXTURE);
        let ide = cands.iter().find(|c| c.pid == 40001).expect("ide server");
        assert_eq!(
            ide.csrf_token.as_deref(),
            Some("1e2d3c4b-aaaa-bbbb-cccc-000000000001")
        );
        assert_eq!(ide.extension_port, Some(42100));
        assert_eq!(
            ide.extension_csrf_token.as_deref(),
            Some("9f8e7d6c-dddd-eeee-ffff-000000000002")
        );
    }

    #[test]
    fn ps_parse_finds_cli_without_token() {
        let cands = candidates_from_ps_output(PS_FIXTURE);
        assert!(cands.iter().any(|c| c.pid == 40003 && c.csrf_token.is_none()));
        assert!(cands.iter().any(|c| c.pid == 40004 && c.csrf_token.is_none()));
    }

    #[test]
    fn ps_parse_skips_non_antigravity_and_tokenless_ide() {
        // Codeium's language server (40002) is not Antigravity; crashpad and
        // systemd are not language servers.
        let cands = candidates_from_ps_output(PS_FIXTURE);
        assert!(!cands.iter().any(|c| c.pid == 40002));
        assert!(!cands.iter().any(|c| c.pid == 16970));
        // A tokenless IDE language server must be skipped, never probed.
        let tokenless =
            "50 /Applications/Antigravity IDE.app/x/language_server_macos_arm --app_data_dir antigravity";
        assert!(candidates_from_ps_output(tokenless).is_empty());
    }

    #[test]
    fn ps_parse_supports_equals_style_flags() {
        let line = "60 /apps/antigravity/language_server --csrf_token=tok-123 --extension_server_port=4242";
        let cands = candidates_from_ps_output(line);
        assert_eq!(cands[0].csrf_token.as_deref(), Some("tok-123"));
        assert_eq!(cands[0].extension_port, Some(4242));
    }

    #[test]
    fn lsof_parse_extracts_listen_ports() {
        let out = "\
COMMAND     PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
language_ 40001   me   23u  IPv4 0x0        0t0  TCP 127.0.0.1:42100 (LISTEN)
language_ 40001   me   24u  IPv4 0x0        0t0  TCP 127.0.0.1:42101 (LISTEN)
language_ 40001   me   25u  IPv6 0x0        0t0  TCP [::1]:42102 (LISTEN)
language_ 40001   me   26u  IPv4 0x0        0t0  TCP *:42103 (LISTEN)
language_ 40001   me   27u  IPv4 0x0        0t0  TCP 127.0.0.1:42100 (LISTEN)
";
        assert_eq!(ports_from_lsof_output(out), vec![42100, 42101, 42102, 42103]);
    }

    #[test]
    fn lsof_parse_empty_output_is_empty() {
        assert!(ports_from_lsof_output("").is_empty());
    }
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test --lib antigravity::fetch`
Expected: compile error — functions not defined. (Add `pub mod fetch;` to `mod.rs` now.)

- [ ] **Step 3: Implement the parsers** — top of `src/antigravity/fetch.rs`, above the tests:

```rust
//! Discovery + fetch for the Antigravity language server.
//!
//! Discovery is two subprocess calls (`ps`, `lsof`) whose OUTPUT PARSING is
//! pure and unit-tested; the subprocess execution itself is a thin shell.
//! The quota fetch is Connect-RPC-over-HTTPS JSON POSTs against loopback.
//! The IDE's server authenticates local requests with the `--csrf_token`
//! from its own command line; the CLI (`agy`) server needs no token.

use crate::usage::AntigravitySnapshot;

/// One plausible Antigravity language-server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCandidate {
    pub pid: u32,
    /// None ⇒ CLI-style server (no CSRF required). A tokenless IDE server is
    /// never a candidate — it would 401 every probe.
    pub csrf_token: Option<String>,
    /// Direct port shortcut from `--extension_server_port`, when present.
    pub extension_port: Option<u16>,
    pub extension_csrf_token: Option<String>,
}

fn extract_flag(cmd: &str, flag: &str) -> Option<String> {
    let mut parts = cmd.split_whitespace().peekable();
    while let Some(p) = parts.next() {
        if p == flag {
            return parts.peek().map(|s| s.to_string());
        }
        if let Some(v) = p.strip_prefix(&format!("{flag}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn is_language_server(cmd_lower: &str) -> bool {
    cmd_lower.contains("language_server")
}

fn is_antigravity(cmd_lower: &str) -> bool {
    cmd_lower.contains("antigravity")
}

/// The CLI's embedded server: the `agy` binary or `antigravity-cli`/`_cli`.
fn is_cli(cmd_lower: &str) -> bool {
    let bin = cmd_lower.split_whitespace().next().unwrap_or("");
    bin.ends_with("/agy") || bin == "agy" || cmd_lower.contains("antigravity-cli")
        || cmd_lower.contains("antigravity_cli")
}

/// Parse `ps -ax -o pid=,command=` output into candidates.
pub fn candidates_from_ps_output(out: &str) -> Vec<ProcessCandidate> {
    let mut cands = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        let Some((pid_s, cmd)) = trimmed.split_once(' ') else {
            continue;
        };
        let Ok(pid) = pid_s.trim().parse::<u32>() else {
            continue;
        };
        let lower = cmd.to_lowercase();
        if is_language_server(&lower) && is_antigravity(&lower) {
            // IDE/app server — must carry a CSRF token or it can't be probed.
            let Some(token) = extract_flag(cmd, "--csrf_token") else {
                continue;
            };
            cands.push(ProcessCandidate {
                pid,
                csrf_token: Some(token),
                extension_port: extract_flag(cmd, "--extension_server_port")
                    .and_then(|p| p.parse().ok()),
                extension_csrf_token: extract_flag(cmd, "--extension_server_csrf_token"),
            });
        } else if is_cli(&lower) {
            cands.push(ProcessCandidate {
                pid,
                csrf_token: None,
                extension_port: None,
                extension_csrf_token: None,
            });
        }
    }
    cands
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` output into a deduped,
/// order-preserving port list. Accepts `127.0.0.1:P`, `[::1]:P`, and `*:P`.
pub fn ports_from_lsof_output(out: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = Vec::new();
    for line in out.lines() {
        if !line.contains("(LISTEN)") {
            continue;
        }
        let Some(name) = line.split_whitespace().rev().nth(1) else {
            continue;
        };
        let Some(port_s) = name.rsplit(':').next() else {
            continue;
        };
        if let Ok(port) = port_s.parse::<u16>()
            && !ports.contains(&port)
        {
            ports.push(port);
        }
    }
    ports
}
```

Also update `src/antigravity/mod.rs`:

```rust
pub mod fetch;
pub mod types;

pub use fetch::{FetchOutcome, fetch_snapshot};
```

(The `FetchOutcome`/`fetch_snapshot` re-export lands in Task 4 — if the compiler complains now, keep only `pub mod fetch; pub mod types;` and add the re-exports in Task 4.)

- [ ] **Step 4: Run tests**

Run: `cargo test --lib antigravity::fetch`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/antigravity/
git commit -m "feat(antigravity): ps/lsof discovery parsers"
```

---

### Task 4: `fetch.rs` — loopback client, probe, `fetch_snapshot` with cache

**Files:**
- Modify: `src/antigravity/fetch.rs`
- Modify: `src/antigravity/mod.rs` (re-exports)

**Interfaces:**
- Consumes: `types::{QuotaSummaryResponse, UserStatusResponse, snap_to_json, parse_cache}` (Task 2), parsers (Task 3), `crate::cache::{Cache, acquire_lock}`, `crate::error::{AppError, Result}`.
- Produces:
  - `pub struct Endpoint { pub base_url: String, pub csrf_token: Option<String> }`
  - `pub fn loopback_client() -> Result<reqwest::Client>` — the ONLY client with `danger_accept_invalid_certs`
  - `pub async fn discover_endpoints() -> Vec<Endpoint>` — runs ps/lsof (production shell, not unit-tested)
  - `pub struct FetchOutcome { pub snapshot: AntigravitySnapshot, pub stale: bool, pub last_error: Option<(u16, String)>, pub cache_age: Option<Duration> }`
  - `pub async fn fetch_snapshot(client: &reqwest::Client, endpoints: &[Endpoint], cache: &Cache, cache_ttl: Duration) -> Result<FetchOutcome>`

- [ ] **Step 1: Write the failing tests** — append to the test module in `fetch.rs`:

```rust
    use crate::cache::Cache;
    use std::time::Duration;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("antigravity"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    const SUMMARY_BODY: &str = r#"{"groups":[
        {"displayName":"Gemini Models","buckets":[
            {"bucketId":"gemini-5h","displayName":"5-hour limit",
             "remaining":{"remainingFraction":0.37}},
            {"bucketId":"gemini-weekly","displayName":"Weekly limit",
             "remaining":{"remainingFraction":0.72}}
        ]},
        {"displayName":"Claude and GPT models","buckets":[
            {"bucketId":"cg-5h","displayName":"5-hour limit",
             "remaining":{"remainingFraction":0.10}},
            {"bucketId":"cg-weekly","displayName":"Weekly limit",
             "remaining":{"remainingFraction":0.55}}
        ]}
    ]}"#;

    #[tokio::test]
    async fn live_200_parses_summary_and_sends_headers() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "POST",
                "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary",
            )
            .match_header("x-codeium-csrf-token", "tok-1")
            .match_header("connect-protocol-version", "1")
            .with_status(200)
            .with_body(SUMMARY_BODY)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = vec![Endpoint {
            base_url: server.url(),
            csrf_token: Some("tok-1".into()),
        }];
        let out = fetch_snapshot(&client, &endpoints, &cache, Duration::ZERO)
            .await
            .unwrap();
        m.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.snapshot.gemini_session.as_ref().unwrap().utilization_pct, 63);
        assert_eq!(out.snapshot.claude_gpt_session.as_ref().unwrap().utilization_pct, 90);
    }

    #[tokio::test]
    async fn summary_404_falls_back_to_user_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "POST",
                "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary",
            )
            .with_status(404)
            .create_async()
            .await;
        server
            .mock(
                "POST",
                "/exa.language_server_pb.LanguageServerService/GetUserStatus",
            )
            .with_status(200)
            .with_body(
                r#"{"userStatus":{"cascadeModelConfigData":{"clientModelConfigs":[
                    {"label":"Gemini 3 Pro","quotaInfo":{"remainingFraction":0.4}}
                ]}}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = vec![Endpoint {
            base_url: server.url(),
            csrf_token: None,
        }];
        let out = fetch_snapshot(&client, &endpoints, &cache, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(out.snapshot.gemini_session.as_ref().unwrap().utilization_pct, 60);
    }

    #[tokio::test]
    async fn no_endpoints_serves_stale_cache_with_error() {
        let (_td, cache) = cache_fixture();
        let seed = super::super::types::snap_to_json(&{
            let mut s = AntigravitySnapshot::default();
            s.gemini_session = Some(crate::usage::UsageWindow {
                utilization_pct: 42,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            });
            s
        });
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot(&client, &[], &cache, Duration::ZERO)
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.gemini_session.as_ref().unwrap().utilization_pct, 42);
        assert!(out.last_error.is_some());
    }

    #[tokio::test]
    async fn no_endpoints_no_cache_errors_with_hint() {
        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let err = fetch_snapshot(&client, &[], &cache, Duration::ZERO)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Antigravity"), "actionable message: {msg}");
    }

    #[tokio::test]
    async fn fresh_cache_short_circuits_without_network() {
        let (_td, cache) = cache_fixture();
        let seed = super::super::types::snap_to_json(&AntigravitySnapshot::default());
        cache.write_payload(seed.to_string().as_bytes()).unwrap();
        let client = reqwest::Client::new();
        // No mockito server: any HTTP attempt would fail, proving the
        // fresh cache short-circuits.
        let out = fetch_snapshot(&client, &[], &cache, Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn second_endpoint_wins_when_first_is_dead() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "POST",
                "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary",
            )
            .with_status(200)
            .with_body(SUMMARY_BODY)
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = vec![
            // Dead port — reqwest connect error, must move on.
            Endpoint { base_url: "http://127.0.0.1:1".into(), csrf_token: None },
            Endpoint { base_url: server.url(), csrf_token: None },
        ];
        let out = fetch_snapshot(&client, &endpoints, &cache, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(out.snapshot.gemini_session.as_ref().unwrap().utilization_pct, 63);
    }
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test --lib antigravity::fetch`
Expected: compile error — `Endpoint`, `fetch_snapshot` not defined.

- [ ] **Step 3: Implement** — add to `fetch.rs` (below the parsers, above tests):

```rust
use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};

use super::types::{QuotaSummaryResponse, UserStatusResponse, parse_cache, snap_to_json};

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const LS_SERVICE: &str = "/exa.language_server_pb.LanguageServerService";

/// A probe-able base URL + the CSRF token it requires (None for the CLI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base_url: String,
    pub csrf_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: AntigravitySnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// Dedicated loopback client. The language server presents a self-signed
/// certificate, so certificate validation is disabled — acceptable ONLY
/// because every URL this client ever sees is `https://127.0.0.1:…` built
/// by `discover_endpoints`. Never reuse this client for remote hosts.
pub fn loopback_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(crate::vendor::HTTP_CLIENT_TIMEOUT)
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| AppError::Other(format!("antigravity http client init: {e}")))
}

/// Run `ps` + `lsof` and turn the results into probe-able endpoints.
/// Production-only shell around the pure parsers above; returns an empty
/// list when nothing is running (the caller degrades to cache).
pub async fn discover_endpoints() -> Vec<Endpoint> {
    let Ok(ps) = tokio::process::Command::new("ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
        .await
    else {
        return Vec::new();
    };
    let candidates = candidates_from_ps_output(&String::from_utf8_lossy(&ps.stdout));

    let mut endpoints = Vec::new();
    for cand in &candidates {
        // Direct extension-server shortcut first: no lsof needed.
        if let (Some(port), Some(tok)) = (cand.extension_port, &cand.extension_csrf_token) {
            endpoints.push(Endpoint {
                base_url: format!("https://127.0.0.1:{port}"),
                csrf_token: Some(tok.clone()),
            });
        }
        let Ok(lsof) = tokio::process::Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &cand.pid.to_string()])
            .output()
            .await
        else {
            continue;
        };
        for port in ports_from_lsof_output(&String::from_utf8_lossy(&lsof.stdout)) {
            let ep = Endpoint {
                base_url: format!("https://127.0.0.1:{port}"),
                csrf_token: cand.csrf_token.clone(),
            };
            if !endpoints.contains(&ep) {
                endpoints.push(ep);
            }
        }
    }
    endpoints
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    endpoints: &[Endpoint],
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse_cache(bytes, cache, false));
    }

    if endpoints.is_empty() {
        let err = AppError::Other(
            "antigravity: language server not running — open the Antigravity IDE \
             (or start `agy`) so quota can be read."
                .into(),
        );
        cache.mark_stale();
        cache.write_last_error(0, &err.to_string());
        return fallback_with_error(cache, err);
    }

    let mut last: Option<AppError> = None;
    for ep in endpoints {
        match fetch_live(client, ep).await {
            Ok(snap) => {
                let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap_or_default();
                cache.write_payload(&bytes)?;
                return Ok(FetchOutcome {
                    snapshot: snap,
                    stale: false,
                    last_error: None,
                    cache_age: Some(Duration::ZERO),
                });
            }
            Err(e) => last = Some(e),
        }
    }
    let err = last.unwrap_or_else(|| AppError::Other("antigravity: no endpoint answered".into()));
    // Every port refused/failed while a process exists — usually the server
    // is still starting. Treat as transient-ish: keep cache, note the error.
    cache.mark_stale();
    cache.write_last_error(0, &err.to_string());
    fallback_with_error(cache, err)
}

/// One endpoint: try the preferred quota summary, then the legacy status.
async fn fetch_live(client: &reqwest::Client, ep: &Endpoint) -> Result<AntigravitySnapshot> {
    match post_json(client, ep, "RetrieveUserQuotaSummary").await {
        Ok(bytes) => {
            let resp: QuotaSummaryResponse = serde_json::from_slice(&bytes)
                .map_err(|e| AppError::Schema(format!("antigravity quota summary: {e}")))?;
            Ok(resp.into_snapshot())
        }
        Err(_) => {
            let bytes = post_json(client, ep, "GetUserStatus").await?;
            let resp: UserStatusResponse = serde_json::from_slice(&bytes)
                .map_err(|e| AppError::Schema(format!("antigravity user status: {e}")))?;
            Ok(resp.into_snapshot())
        }
    }
}

async fn post_json(client: &reqwest::Client, ep: &Endpoint, method: &str) -> Result<Vec<u8>> {
    let url = format!("{}{LS_SERVICE}/{method}", ep.base_url);
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}");
    if let Some(tok) = &ep.csrf_token {
        req = req.header("X-Codeium-Csrf-Token", tok);
    }
    let resp = tokio::time::timeout(HTTP_TIMEOUT, req.send())
        .await
        .map_err(|_| AppError::Transport(format!("antigravity timeout: {url}")))??;
    let status = resp.status();
    let bytes = resp.bytes().await?.to_vec();
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    Ok(bytes)
}

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
```

Update `src/antigravity/mod.rs` re-exports:

```rust
pub use fetch::{Endpoint, FetchOutcome, discover_endpoints, fetch_snapshot, loopback_client};
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib antigravity`
Expected: PASS (parsers + 6 new async tests).

Note: mockito serves plain `http://` — fine, the scheme lives in the injected `base_url`; production builds `https://` in `discover_endpoints`.

Note (deliberate spec simplification): the spec's port-validation step (`GetUnleashData` probe per port) is folded into the quota fetch itself — probing `RetrieveUserQuotaSummary` directly on each candidate port validates and fetches in one round-trip. Same outcome, one fewer request per port.

- [ ] **Step 5: Commit**

```bash
git add src/antigravity/
git commit -m "feat(antigravity): loopback probe + fetch_snapshot with cache fallback"
```

---

### Task 5: `src/antigravity/vendor.rs` — renderer + placeholders

**Files:**
- Create: `src/antigravity/vendor.rs`
- Modify: `src/antigravity/mod.rs` (add `pub mod vendor;`)

**Interfaces:**
- Consumes: `AntigravitySnapshot` (Task 1), `FetchOutcome` (Task 4), shared `format/pango/tooltip/theme/countdown` helpers (same imports as `src/zai/vendor.rs`).
- Produces: `pub const DEFAULT_FORMAT`, `pub fn build_placeholders(&AntigravitySnapshot, DateTime<Utc>) -> HashMap<&'static str, String>`, `pub fn severity(&AntigravitySnapshot) -> PaceSeverity`, `pub fn render(&VendorOutcome, &AntigravitySnapshot, &Theme, &RenderOpts, DateTime<Utc>) -> WaybarOutput`, `impl From<FetchOutcome> for VendorOutcome`.
- Placeholder contract (consumed by the macOS menu bar's fixed FORMAT string): `plan`, `session_pct`/`session_reset` ← Gemini 5h, `weekly_pct`/`weekly_reset` ← Gemini weekly, `sonnet_pct`/`sonnet_reset` ← Claude+GPT 5h, `extra_pct`/`extra_spent`/`extra_limit` ← Claude+GPT 5h (spent = "N%", limit = "100%"). Vendor-specific: `ag_gem_5h_pct`, `ag_gem_5h_reset`, `ag_gem_week_pct`, `ag_gem_week_reset`, `ag_cg_5h_pct`, `ag_cg_5h_reset`, `ag_cg_week_pct`, `ag_cg_week_reset`.

- [ ] **Step 1: Write the failing tests** — test module of the new `vendor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{AntigravitySnapshot, UsageWindow};

    fn w(pct: i32) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: Some(Utc::now() + chrono::Duration::hours(2)),
            window_duration: chrono::Duration::hours(5),
        }
    }

    fn sample_snap() -> AntigravitySnapshot {
        AntigravitySnapshot {
            gemini_session: Some(w(63)),
            gemini_weekly: Some(w(28)),
            claude_gpt_session: Some(w(90)),
            claude_gpt_weekly: Some(w(45)),
        }
    }

    fn outcome(s: AntigravitySnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Antigravity(s),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(10)),
        }
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

    #[test]
    fn default_format_shows_gemini_pcts() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("63%"), "text: {}", out.text);
        assert!(out.text.contains("28%"), "text: {}", out.text);
    }

    #[test]
    fn cross_vendor_aliases_cover_menubar_contract() {
        let values = build_placeholders(&sample_snap(), Utc::now());
        assert_eq!(values.get("session_pct").map(String::as_str), Some("63"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("28"));
        // Claude+GPT 5h rides the sonnet + extra slots so the macOS menu bar
        // and GNOME extension render a third bar without vendor-specific code.
        assert_eq!(values.get("sonnet_pct").map(String::as_str), Some("90"));
        assert_eq!(values.get("extra_pct").map(String::as_str), Some("90"));
        assert_eq!(values.get("extra_spent").map(String::as_str), Some("90%"));
        assert_eq!(values.get("extra_limit").map(String::as_str), Some("100%"));
        assert_eq!(values.get("plan").map(String::as_str), Some("Antigravity"));
    }

    #[test]
    fn missing_buckets_render_zero_and_dash() {
        let values = build_placeholders(&AntigravitySnapshot::default(), Utc::now());
        assert_eq!(values.get("session_pct").map(String::as_str), Some("0"));
        assert_eq!(values.get("session_reset").map(String::as_str), Some("—"));
        // No Claude+GPT bucket → no third bar (empty spent/limit make the
        // Swift/GNOME parsers treat extra as absent).
        assert_eq!(values.get("extra_spent").map(String::as_str), Some(""));
        assert_eq!(values.get("extra_limit").map(String::as_str), Some(""));
    }

    #[test]
    fn tooltip_lists_all_four_buckets() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("Gemini 5h"));
        assert!(out.tooltip.contains("Gemini weekly"));
        assert!(out.tooltip.contains("Claude+GPT 5h"));
        assert!(out.tooltip.contains("Claude+GPT weekly"));
        assert!(out.tooltip.contains("Updated"));
    }

    #[test]
    fn empty_snapshot_renders_no_buckets_message() {
        let snap = AntigravitySnapshot::default();
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("no quota buckets reported"));
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut oc = outcome(snap.clone());
        oc.stale = true;
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn severity_picks_worst_bucket() {
        assert_eq!(severity(&sample_snap()), PaceSeverity::Critical); // 90
        assert_eq!(severity(&AntigravitySnapshot::default()), PaceSeverity::Low);
    }

    #[test]
    fn custom_format_uses_vendor_placeholders() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let mut o = opts();
        o.format = Some("G:{ag_gem_5h_pct} C:{ag_cg_5h_pct}".into());
        let out = render(&oc, &snap, &Theme::default(), &o, Utc::now());
        assert!(out.text.contains("G:63 C:90"), "{}", out.text);
    }
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test --lib antigravity::vendor`
Expected: compile error.

- [ ] **Step 3: Implement `src/antigravity/vendor.rs`** (mirror of `src/zai/vendor.rs`):

```rust
//! Antigravity renderer — bar text + bordered Pango tooltip, one progress
//! bar per quota bucket (Gemini 5h/weekly, Claude+GPT 5h/weekly).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{self, color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::{AntigravitySnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{ag_gem_5h_pct}% · {ag_gem_week_pct}%";

fn pct(w: &Option<UsageWindow>) -> i32 {
    w.as_ref().map(|w| w.utilization_pct).unwrap_or(0)
}

fn reset(w: &Option<UsageWindow>, now: DateTime<Utc>) -> String {
    countdown::format(w.as_ref().and_then(|w| w.resets_at), now)
}

pub fn build_placeholders(
    snap: &AntigravitySnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let cg5 = &snap.claude_gpt_session;
    // The money-bucket aliases (Anthropic's "extra usage" slot) carry the
    // Claude+GPT 5h bucket so the GNOME extension and macOS menu bar render
    // a third bar without antigravity-specific wiring. Empty spent/limit
    // signal "no bucket" to those parsers.
    let (extra_spent, extra_limit) = match cg5 {
        Some(w) => (format!("{}%", w.utilization_pct), "100%".to_string()),
        None => (String::new(), String::new()),
    };
    placeholders(vec![
        ("icon", "󰊭".to_string()),
        ("vendor_short", "ag".to_string()),
        ("plan", "Antigravity".to_string()),
        // Cross-vendor aliases — Gemini is the primary group.
        ("session_pct", pct(&snap.gemini_session).to_string()),
        ("session_reset", reset(&snap.gemini_session, now)),
        ("weekly_pct", pct(&snap.gemini_weekly).to_string()),
        ("weekly_reset", reset(&snap.gemini_weekly, now)),
        // Claude+GPT 5h rides the sonnet + extra slots (third bar).
        ("sonnet_pct", pct(cg5).to_string()),
        ("sonnet_reset", reset(cg5, now)),
        ("extra_pct", pct(cg5).to_string()),
        ("extra_spent", extra_spent),
        ("extra_limit", extra_limit),
        // Vendor-specific placeholders.
        ("ag_gem_5h_pct", pct(&snap.gemini_session).to_string()),
        ("ag_gem_5h_reset", reset(&snap.gemini_session, now)),
        ("ag_gem_week_pct", pct(&snap.gemini_weekly).to_string()),
        ("ag_gem_week_reset", reset(&snap.gemini_weekly, now)),
        ("ag_cg_5h_pct", pct(cg5).to_string()),
        ("ag_cg_5h_reset", reset(cg5, now)),
        ("ag_cg_week_pct", pct(&snap.claude_gpt_weekly).to_string()),
        ("ag_cg_week_reset", reset(&snap.claude_gpt_weekly, now)),
    ])
}

pub fn severity(snap: &AntigravitySnapshot) -> PaceSeverity {
    severity_for(snap.max_pct())
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &AntigravitySnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, now);

    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let wrapper_color = severity_color(severity(snap), theme).to_string();
    let icon_prefix = match opts.icon.as_deref() {
        Some(ic) if !ic.is_empty() => format!("{ic} "),
        _ => String::new(),
    };
    let bar_text = color_span(&wrapper_color, &format!("{icon_prefix}{text}"));

    let tooltip = if let Some(fmt) = opts.tooltip_format.as_deref() {
        substitute(fmt, &values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &AntigravitySnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Antigravity</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    let buckets: [(&str, &Option<UsageWindow>); 4] = [
        ("  󰔟  Gemini 5h", &snap.gemini_session),
        ("  󰃰  Gemini weekly", &snap.gemini_weekly),
        ("  󰔟  Claude+GPT 5h", &snap.claude_gpt_session),
        ("  󰃰  Claude+GPT weekly", &snap.claude_gpt_weekly),
    ];
    let mut any = false;
    for (label, w) in buckets {
        if let Some(w) = w {
            if any {
                lines.push(TooltipLine::Body("".into()));
            }
            push_window(&mut lines, label, w, theme, now);
            any = true;
        }
    }
    if !any {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>no quota buckets reported</span>"
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref()
        && *code != 0
    {
        let (icon, ecolor) = if *code >= 500 {
            ("󰅚", theme.red.as_str())
        } else {
            ("󰀪", theme.orange.as_str())
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  HTTP {code}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{dim}'>{}</span>",
            escape(msg)
        )));
    }

    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

fn push_window(
    lines: &mut Vec<TooltipLine>,
    label: &str,
    w: &UsageWindow,
    theme: &Theme,
    now: DateTime<Utc>,
) {
    let color = severity_color(severity_for(w.utilization_pct), theme);
    let bar = pango::progress_bar(w.utilization_pct, color, theme, None);
    let fg = &theme.fg;
    let dim = &theme.dim;
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>{label}</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>",
        pct = w.utilization_pct
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
        cd = escape(&countdown::format(w.resets_at, now))
    )));
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Antigravity(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}
```

Add `pub mod vendor;` to `src/antigravity/mod.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib antigravity`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/antigravity/
git commit -m "feat(antigravity): renderer with menubar-compatible placeholders"
```

---

### Task 6: Full wiring — `VendorId`, config, CLI, active cycle, widget shell, TUI

**Files:**
- Modify: `src/vendor.rs` (enum + slug + all)
- Modify: `src/config.rs` (`AntigravityConfig`, `Config` field, `is_enabled`)
- Modify: `src/active.rs` (`parse_slug`)
- Modify: `src/widget/cli.rs` (`Vendor` enum + both mappings)
- Modify: `src/widget/run.rs` (dispatch arm + `antigravity_output`)
- Modify: `src/tui/app.rs` (fetch arm)
- Modify: `src/tui/view.rs` (2 label fns)
- Modify: `src/tui/settings.rs` (label fn)

**Interfaces:**
- Consumes: everything from Tasks 1–5 (`antigravity::{discover_endpoints, fetch_snapshot, loopback_client}`, `antigravity::vendor::render`).
- Produces: `VendorId::Antigravity` (slug `"antigravity"`), `config.antigravity.enabled` (default `false`).

- [ ] **Step 1: Write the failing config test** — in `src/config.rs` `mod tests`:

```rust
    #[test]
    fn antigravity_defaults_off_and_parses_enable() {
        let c = Config::default();
        assert!(!c.is_enabled(VendorId::Antigravity));
        let c: Config = toml::from_str("[antigravity]\nenabled = true\n").unwrap();
        assert!(c.is_enabled(VendorId::Antigravity));
        assert!(c.enabled_vendors().contains(&VendorId::Antigravity));
    }
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test --lib config`
Expected: compile error — `VendorId::Antigravity` missing.

- [ ] **Step 3: Wire everything** (one sweep; the compiler's exhaustive-match errors are the checklist):

`src/vendor.rs` — add to enum, `slug()`, `all()`:

```rust
    Antigravity,
```
```rust
            VendorId::Antigravity => "antigravity",
```
```rust
            VendorId::Antigravity,
```

`src/config.rs` — field on `Config`:

```rust
    pub antigravity: AntigravityConfig,
```

struct + default after `OpencodeConfig` (~line 172):

```rust
/// Google Antigravity IDE — no credentials/API key; probes the IDE's local
/// language server, which must be running for fresh data. Quota fractions
/// come straight from the server, so there are no limits to configure.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AntigravityConfig {
    pub enabled: bool,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}
```

`is_enabled` arm:

```rust
            VendorId::Antigravity => self.antigravity.enabled,
```

`src/active.rs` `parse_slug`:

```rust
        "antigravity" => Some(VendorId::Antigravity),
```

`src/widget/cli.rs` — `Vendor` enum, `to_id`, `id_to_vendor`:

```rust
    Antigravity,
```
```rust
            Vendor::Antigravity => crate::vendor::VendorId::Antigravity,
```
```rust
        crate::vendor::VendorId::Antigravity => Vendor::Antigravity,
```

`src/widget/run.rs` — import `use crate::antigravity;`, dispatch arm:

```rust
        Vendor::Antigravity => antigravity_output(cli).await,
```

and the output fn (after `opencode_output`):

```rust
/// Local-first vendor: probes the Antigravity IDE's language server on
/// loopback. Uses the dedicated self-signed-tolerant client, never the
/// shared one.
async fn antigravity_output(cli: &Cli) -> Result<WaybarOutput> {
    let cache = vendor_cache(cli, "antigravity")?;
    let client = antigravity::loopback_client()?;
    let endpoints = antigravity::discover_endpoints().await;
    let outcome =
        match antigravity::fetch_snapshot(&client, &endpoints, &cache, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(antigravity::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        Utc::now(),
    ))
}
```

(If the `build_output` dispatch passes `&config` to every arm, match the existing signature style: `Vendor::Antigravity => antigravity_output(cli, &config).await` with an unused `_config` parameter.)

`src/tui/app.rs` — fetch arm (after the `Opencode` arm, ~line 242):

```rust
        VendorId::Antigravity => {
            let cache = crate::cache::Cache::for_vendor("antigravity")?;
            let lb_client = crate::antigravity::loopback_client()?;
            let endpoints = crate::antigravity::discover_endpoints().await;
            let outcome =
                crate::antigravity::fetch_snapshot(&lb_client, &endpoints, &cache, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
```

`src/tui/view.rs` — both label fns:

```rust
        VendorId::Antigravity => "Antigravity",
```

`src/tui/settings.rs` — `vendor_label`:

```rust
        VendorId::Antigravity => "Antigravity",
```

- [ ] **Step 4: Chase remaining exhaustive-match errors**

Run: `cargo build --all-targets 2>&1 | head -50` and fix every `non-exhaustive patterns` error the same way (there may be arms this plan didn't list — e.g. other `match` sites added since; label them "Antigravity").

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: PASS — including the pre-existing generic tests that iterate `VendorId::all()` (slug round-trip, settings, active-cycle).

- [ ] **Step 6: Manual smoke of the widget shell**

Run: `cargo run --bin ai-usagebar -- --vendor antigravity; echo "exit=$?"`
Expected: exit=0 always. With the IDE closed: either a stale snapshot (⏸) or the pretty-printed error fallback mentioning Antigravity. With the IDE open: four-bucket data.

- [ ] **Step 7: Commit**

```bash
git add src/vendor.rs src/config.rs src/active.rs src/widget/ src/tui/
git commit -m "feat(antigravity): wire vendor into widget, CLI, config, and TUI"
```

---

### Task 7: E2E insta test + live smoke

**Files:**
- Create: `tests/antigravity_e2e.rs`
- Create (generated): `tests/snapshots/antigravity_e2e__antigravity_full_bar_text.snap`, `tests/snapshots/antigravity_e2e__antigravity_full_tooltip.snap`
- Modify: `tests/live.rs` (add `#[ignore]`d smoke)

**Interfaces:**
- Consumes: public crate API — `ai_usagebar::antigravity::{self, Endpoint}`, `ai_usagebar::cache::Cache`, `ai_usagebar::format::updated_at_hm`.

- [ ] **Step 1: Write `tests/antigravity_e2e.rs`**

```rust
//! End-to-end integration test for the Antigravity vendor.
//!
//! Serves a canned `RetrieveUserQuotaSummary` response via mockito, drives
//! the full `fetch_snapshot` + `antigravity::vendor::render` pipeline, and
//! asserts the resulting Waybar JSON via `insta` snapshots. The fixture
//! doubles as a canary for language-server response drift.

use std::time::Duration;

use ai_usagebar::antigravity::{self, Endpoint};
use ai_usagebar::cache::Cache;
use ai_usagebar::format::updated_at_hm;
use ai_usagebar::theme::Theme;
use ai_usagebar::vendor::{RenderOpts, VendorOutcome};
use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap()
}

const SUMMARY_BODY: &str = r#"{"groups":[
    {"displayName":"Gemini Models","buckets":[
        {"bucketId":"gemini-5h","displayName":"5-hour limit",
         "remaining":{"remainingFraction":0.37},
         "resetTime":"2026-07-01T14:30:00Z"},
        {"bucketId":"gemini-weekly","displayName":"Weekly limit",
         "remaining":{"remainingFraction":0.72},
         "resetTime":"2026-07-05T00:00:00Z"}
    ]},
    {"displayName":"Claude and GPT models","buckets":[
        {"bucketId":"cg-5h","displayName":"5-hour limit",
         "remaining":{"remainingFraction":0.10},
         "resetTime":"2026-07-01T14:30:00Z"},
        {"bucketId":"cg-weekly","displayName":"Weekly limit",
         "remaining":{"remainingFraction":0.55},
         "resetTime":"2026-07-05T00:00:00Z"}
    ]}
]}"#;

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
async fn full_summary_renders_expected_waybar_json() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock(
            "POST",
            "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary",
        )
        .with_status(200)
        .with_body(SUMMARY_BODY)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = Cache::at(td.path().join("cache").join("antigravity"));
    let client = reqwest::Client::new();
    let endpoints = vec![Endpoint {
        base_url: server.url(),
        csrf_token: Some("tok-e2e".into()),
    }];

    let outcome = antigravity::fetch_snapshot(&client, &endpoints, &cache, Duration::ZERO)
        .await
        .unwrap();

    // Gemini 63%/28%, Claude+GPT 90%/45% → worst bucket Critical.
    let snap = outcome.snapshot.clone();
    let cache_age = outcome.cache_age;
    let vendor_outcome: VendorOutcome = outcome.into();
    let theme = Theme::default(); // One Dark — stable across machines
    let out = antigravity::vendor::render(&vendor_outcome, &snap, &theme, &opts(), now());

    insta::assert_snapshot!("antigravity_full_bar_text", &out.text);
    insta::assert_snapshot!(
        "antigravity_full_tooltip",
        normalize_updated_time(&out.tooltip, now(), cache_age)
    );
    assert_eq!(format!("{:?}", out.class), "Critical");
}

#[tokio::test]
async fn no_server_with_no_cache_is_a_credible_widget_error() {
    let td = TempDir::new().unwrap();
    let cache = Cache::at(td.path().join("cache").join("antigravity"));
    let client = reqwest::Client::new();
    let err = antigravity::fetch_snapshot(&client, &[], &cache, Duration::ZERO)
        .await
        .unwrap_err();
    // The widget shell turns this into the ⚠ fallback; the message must tell
    // the user to open the IDE.
    let msg = err.to_string();
    assert!(msg.contains("Antigravity"), "{msg}");
    assert!(msg.contains("IDE") || msg.contains("agy"), "{msg}");
}
```

- [ ] **Step 2: Generate + review snapshots**

Run: `INSTA_UPDATE=always cargo test --test antigravity_e2e`
Then inspect `tests/snapshots/antigravity_e2e__*.snap` — bar text must show `63% · 28%`, tooltip must show the four buckets with correct percentages and reset countdowns (relative to the frozen `now()` = 2026-07-01 12:00 UTC: "2h 30m" for 14:30, "3d 12h" for 07-05).
Then run plain `cargo test --test antigravity_e2e` — PASS with committed snapshots.

- [ ] **Step 3: Add the live smoke** — append to `tests/live.rs`, following the existing `#[ignore]` pattern in that file:

```rust
/// Live probe of a locally running Antigravity IDE / agy. Run with:
/// `cargo test --test live antigravity -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn antigravity_live_probe() {
    let endpoints = ai_usagebar::antigravity::discover_endpoints().await;
    assert!(
        !endpoints.is_empty(),
        "no Antigravity language server found — open the IDE first"
    );
    let td = tempfile::TempDir::new().unwrap();
    let cache = ai_usagebar::cache::Cache::at(td.path().join("antigravity"));
    let client = ai_usagebar::antigravity::loopback_client().unwrap();
    let out = ai_usagebar::antigravity::fetch_snapshot(
        &client,
        &endpoints,
        &cache,
        std::time::Duration::ZERO,
    )
    .await
    .unwrap();
    println!("snapshot: {:#?}", out.snapshot);
    assert!(
        out.snapshot.gemini_session.is_some() || out.snapshot.claude_gpt_session.is_some(),
        "expected at least one quota bucket from the live server"
    );
}
```

(Match `tests/live.rs`'s existing import/style conventions exactly — inspect the file first.)

- [ ] **Step 4: Run the live smoke once** (IDE open on this machine):

Run: `cargo test --test live antigravity -- --ignored --nocapture`
Expected: PASS with real bucket data printed. **If the response shape differs from the fixture** (field names, nesting, reset-time format), fix `src/antigravity/types.rs` + fixtures to match reality, re-run Tasks 2/4/7 tests, and update the spec's Types section note.

- [ ] **Step 5: Commit**

```bash
git add tests/antigravity_e2e.rs tests/snapshots/antigravity_e2e__*.snap tests/live.rs
git commit -m "test(antigravity): e2e insta snapshots + live smoke"
```

---

### Task 8: macOS menu bar app

**Files:**
- Modify: `macos/ai-usagebar-menubar.swift`
- Regenerate: `macos/ai-usagebar-menubar` (binary, via `macos/build.sh`)

**Interfaces:**
- Consumes: the placeholder contract from Task 5 (`session_*` = Gemini 5h, `weekly_*` = Gemini weekly, `sonnet_*` = Claude+GPT 5h, `extra_*` = Claude+GPT 5h with `spent`="N%", `limit`="100%").

- [ ] **Step 1: Add the vendor to `VENDOR_AUTH`** (after the opencode entry, ~line 195):

```swift
    // "local" = no credentials; quota is probed from the Antigravity IDE's
    // local language server. Configured ⇔ the IDE (or its CLI dir) exists.
    VendorAuth(id: "antigravity", name: "Antigravity (Google)", kind: "local", cli: "agy", login: "", pkg: "", env: ""),
```

- [ ] **Step 2: `vendorConfigured`** (after the opencode branch, ~line 235):

```swift
    if v.id == "antigravity" {
        return fm.fileExists(atPath: "/Applications/Antigravity IDE.app")
            || fm.fileExists(atPath: "/Applications/Antigravity.app")
            || fm.fileExists(atPath: "\(home)/.antigravity")
    }
```

- [ ] **Step 3: Preferences picker** (~line 379):

```swift
    private let vendors = ["anthropic", "openai", "zai", "openrouter", "deepseek", "opencode", "antigravity"]
```

- [ ] **Step 4: Panel + menu labels** — in `renderPanel` (~line 559) replace the extra seg line:

```swift
        if SHOW_EXTRA, let e = s.extra {
            let tag = VENDOR == "opencode" ? "mo" : (VENDOR == "antigravity" ? "cg" : "ex")
            seg(tag, e.pct, e.spent)
        }
```

In `renderMenu` (~line 580): relabel the sonnet row and suppress the redundant extra row for antigravity (the Claude+GPT bucket already occupies the sonnet row; extra only feeds the compact panel):

```swift
        if let sn = s.sonnet {
            let label = VENDOR == "antigravity" ? "Claude+GPT" : "Sonnet only"
            row("sonnet", label, sn.pct, "\(sn.pct)%", sn.reset)
        }
        else { rows["sonnet"]?.isHidden = true }
        if let e = s.extra, VENDOR != "antigravity" {
            let label = VENDOR == "opencode" ? "Monthly" : "Extra usage"
            row("extra", label, e.pct, "\(e.spent) / \(e.limit)", nil)
        } else { rows["extra"]?.isHidden = true }
```

(The existing sonnet row line is `if let sn = s.sonnet { row("sonnet", "Sonnet only", sn.pct, "\(sn.pct)%", sn.reset) }` — replace it with the block above, preserving surrounding code.)

- [ ] **Step 5: Rebuild and verify**

Run: `cd macos && ./build.sh`
Expected: compiles, binary replaced. Then run it pointing at the antigravity vendor (set the vendor in Preferences or defaults):

```bash
defaults write br.com.ai-usagebar.menubar vendor antigravity 2>/dev/null || true
./ai-usagebar-menubar &
```

(Check the actual defaults domain/key names used in the Swift file — `@AppStorage`/`DEF` keys — and use those; the Preferences window's vendor picker is the fallback verification path.) With the IDE open: three bars (5h, 7d, cg). Kill the test instance afterward.

- [ ] **Step 6: Commit**

```bash
git add macos/ai-usagebar-menubar.swift macos/ai-usagebar-menubar
git commit -m "feat(macos): add Antigravity to menu bar app"
```

---

### Task 9: Docs, example config, gate

**Files:**
- Modify: `config.example.toml` (new section after `[opencode]`, and the `primary` comment line 11)
- Modify: `README.md` (vendor list/table + a short Antigravity subsection)
- Modify: `CHANGELOG.md` (`[Unreleased]` → Added)
- Modify: `CLAUDE.md` ("What lives where" list)

- [ ] **Step 1: `config.example.toml`** — update the primary comment:

```toml
# Valid: anthropic | openai | zai | openrouter | deepseek | opencode | antigravity
```

and append after the `[opencode]` block:

```toml
[antigravity]
# Google Antigravity IDE quota tracking. No credentials — probes the IDE's
# local language server (loopback HTTPS), so the IDE (or the agy CLI) must
# be running for fresh data; otherwise the last cached snapshot is shown.
# Buckets: Gemini 5h/weekly + Claude+GPT 5h/weekly.
enabled = false
```

- [ ] **Step 2: `README.md`** — add `antigravity` wherever the vendor set is enumerated (feature table, `--vendor` docs, waybar config examples), plus a short subsection mirroring the OpenCode one:

```markdown
### Google Antigravity

No credentials needed. The widget probes the Antigravity IDE's local
language server for the plan quota buckets shown in Settings → Models
(Gemini 5h/weekly, Claude+GPT 5h/weekly). The IDE (or the `agy` CLI) must
be running for fresh data; otherwise the cached snapshot is served with a
stale marker. Enable with `[antigravity] enabled = true`.
```

(Adapt placement/wording to the README's existing structure — read the OpenCode section first and mirror it.)

- [ ] **Step 3: `CHANGELOG.md`** — under `## [Unreleased]` → `### Added`:

```markdown
- New `antigravity` vendor: Google Antigravity IDE plan quota (Gemini and
  Claude+GPT groups, 5h + weekly windows) probed from the IDE's local
  language server — no credentials required. Wired into the Waybar widget,
  TUI, and macOS menu bar app.
```

- [ ] **Step 4: `CLAUDE.md`** — add to "What lives where":

```markdown
- `src/antigravity/` — Antigravity vendor: probes the IDE's local language
  server (loopback HTTPS + CSRF token from the process args). The reqwest
  client with `danger_accept_invalid_certs` lives ONLY here
  (`fetch::loopback_client`) and must never touch remote hosts.
```

- [ ] **Step 5: Full gate**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo machete
```

Expected: all green. Fix anything that isn't before committing.

- [ ] **Step 6: Commit**

```bash
git add config.example.toml README.md CHANGELOG.md CLAUDE.md
git commit -m "docs(antigravity): example config, README, CHANGELOG, CLAUDE.md"
```

---

## Not in this plan (deliberate)

- **No release** — version bump / PKGBUILDs / tag follow the CLAUDE.md release checklist as a separate act, once the user decides to cut a version.
- **No OAuth remote fetch, no `agy` PTY spawning, no Gemini CLI vendor** — out of scope per the spec.
- **GNOME extension** — gets the third bar for free via the `extra_*` placeholders; no code change planned. If its parser rejects `"90%" / "100%"` strings, treat as a follow-up.
