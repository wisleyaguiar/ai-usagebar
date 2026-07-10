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
            s.parse::<i64>()
                .ok()
                .and_then(|secs| Utc.timestamp_opt(secs, 0).single())
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
        assert_eq!(
            resp.into_snapshot(),
            crate::usage::AntigravitySnapshot::default()
        );
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
