//! Projection types for the opencode DB aggregation + cache serialization.
//!
//! There is no HTTP wire format here — the "wire" is the opencode CLI's
//! `message` table, whose `data` JSON column carries `cost` (USD),
//! `tokens.total`, `providerID`, and `modelID` per assistant message.

use crate::config::OpencodeConfig;
use crate::error::Result;
use crate::usage::OpencodeSnapshot;

/// Dollar caps for the three rolling windows, straight from `[opencode]`
/// config (they are not discoverable via any API).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub five_hour: f64,
    pub week: f64,
    pub month: f64,
}

impl Limits {
    pub fn from_config(c: &OpencodeConfig) -> Self {
        Self {
            five_hour: c.limit_5h,
            week: c.limit_week,
            month: c.limit_month,
        }
    }
}

/// Raw aggregation result from the DB query, before limits are attached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowTotals {
    pub spent_5h: f64,
    pub spent_week: f64,
    pub spent_month: f64,
    pub tokens_month: i64,
    pub top_model: Option<String>,
}

impl WindowTotals {
    pub fn into_snapshot(self, provider: &str, limits: &Limits) -> OpencodeSnapshot {
        OpencodeSnapshot {
            provider: provider.to_string(),
            spent_5h: self.spent_5h,
            limit_5h: limits.five_hour,
            spent_week: self.spent_week,
            limit_week: limits.week,
            spent_month: self.spent_month,
            limit_month: limits.month,
            tokens_month: self.tokens_month,
            top_model: self.top_model,
        }
    }
}

/// Serialize a snapshot for the per-vendor cache file.
pub fn snap_to_json(snap: &OpencodeSnapshot) -> serde_json::Value {
    serde_json::json!({
        "provider": snap.provider,
        "spent_5h": snap.spent_5h,
        "limit_5h": snap.limit_5h,
        "spent_week": snap.spent_week,
        "limit_week": snap.limit_week,
        "spent_month": snap.spent_month,
        "limit_month": snap.limit_month,
        "tokens_month": snap.tokens_month,
        "top_model": snap.top_model,
    })
}

/// Parse a cached payload back into a snapshot. Missing fields default —
/// same forgiving posture as the other vendors' `parse_cache`.
pub fn parse_cache(bytes: &[u8]) -> Result<OpencodeSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    Ok(OpencodeSnapshot {
        provider: v["provider"].as_str().unwrap_or("").to_string(),
        spent_5h: v["spent_5h"].as_f64().unwrap_or(0.0),
        limit_5h: v["limit_5h"].as_f64().unwrap_or(0.0),
        spent_week: v["spent_week"].as_f64().unwrap_or(0.0),
        limit_week: v["limit_week"].as_f64().unwrap_or(0.0),
        spent_month: v["spent_month"].as_f64().unwrap_or(0.0),
        limit_month: v["limit_month"].as_f64().unwrap_or(0.0),
        tokens_month: v["tokens_month"].as_i64().unwrap_or(0),
        top_model: v["top_model"].as_str().map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            five_hour: 12.0,
            week: 30.0,
            month: 60.0,
        }
    }

    #[test]
    fn totals_project_into_snapshot_with_limits() {
        let totals = WindowTotals {
            spent_5h: 1.5,
            spent_week: 7.25,
            spent_month: 44.87,
            tokens_month: 389_769_308,
            top_model: Some("deepseek-v4-pro".into()),
        };
        let snap = totals.into_snapshot("opencode-go", &limits());
        assert_eq!(snap.provider, "opencode-go");
        assert_eq!(snap.spent_5h, 1.5);
        assert_eq!(snap.limit_5h, 12.0);
        assert_eq!(snap.spent_week, 7.25);
        assert_eq!(snap.limit_week, 30.0);
        assert_eq!(snap.spent_month, 44.87);
        assert_eq!(snap.limit_month, 60.0);
        assert_eq!(snap.tokens_month, 389_769_308);
        assert_eq!(snap.top_model.as_deref(), Some("deepseek-v4-pro"));
    }

    #[test]
    fn cache_round_trip_preserves_snapshot() {
        let snap = WindowTotals {
            spent_5h: 0.5,
            spent_week: 3.0,
            spent_month: 12.0,
            tokens_month: 1000,
            top_model: Some("big-pickle".into()),
        }
        .into_snapshot("opencode-go", &limits());
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        let parsed = parse_cache(&bytes).unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn parse_cache_defaults_missing_fields() {
        let parsed = parse_cache(b"{}").unwrap();
        assert_eq!(parsed, OpencodeSnapshot::default());
    }

    #[test]
    fn limits_from_config_copies_caps() {
        let c = OpencodeConfig {
            limit_5h: 24.0,
            ..Default::default()
        };
        let l = Limits::from_config(&c);
        assert_eq!(l.five_hour, 24.0);
        assert_eq!(l.week, 30.0);
        assert_eq!(l.month, 60.0);
    }
}
