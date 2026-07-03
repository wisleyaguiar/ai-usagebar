//! OpenCode Go renderer — bar text + bordered Pango tooltip with one
//! progress bar per dollar-limit window.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, progress_bar, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::OpencodeSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{oc_5h_pct}% · {oc_week_pct}%";

pub fn build_placeholders(snap: &OpencodeSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", "󰅩".to_string()),
        ("vendor_short", "oc".to_string()),
        // Cross-vendor aliases — 5h/weekly map naturally; rolling windows
        // have no reset instant, so the reset placeholders render "—".
        ("session_pct", snap.pct_5h().to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", snap.pct_week().to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "OpenCode Go".to_string()),
        // Money-bucket aliases (Anthropic's "extra usage" slot) → monthly
        // window, so the GNOME extension and macOS menu bar app can render a
        // third row without knowing opencode-specific placeholders.
        ("extra_pct", snap.pct_month().to_string()),
        ("extra_spent", dollars(snap.spent_month)),
        ("extra_limit", dollars(snap.limit_month)),
        ("oc_5h_spent", dollars(snap.spent_5h)),
        ("oc_5h_limit", dollars(snap.limit_5h)),
        ("oc_5h_pct", snap.pct_5h().to_string()),
        ("oc_week_spent", dollars(snap.spent_week)),
        ("oc_week_limit", dollars(snap.limit_week)),
        ("oc_week_pct", snap.pct_week().to_string()),
        ("oc_month_spent", dollars(snap.spent_month)),
        ("oc_month_limit", dollars(snap.limit_month)),
        ("oc_month_pct", snap.pct_month().to_string()),
        ("oc_tokens_month", compact_tokens(snap.tokens_month)),
        (
            "oc_top_model",
            snap.top_model.clone().unwrap_or_else(|| "—".to_string()),
        ),
    ])
}

fn dollars(v: f64) -> String {
    format!("${v:.2}")
}

/// Compact token counts for the bar/tooltip ("1.5k", "389.8M").
pub(crate) fn compact_tokens(n: i64) -> String {
    let f = n as f64;
    if n >= 1_000_000_000 {
        format!("{:.1}B", f / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1e3)
    } else {
        n.to_string()
    }
}

pub fn severity(snap: &OpencodeSnapshot) -> PaceSeverity {
    severity_for(snap.max_pct())
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &OpencodeSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap);

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
    snap: &OpencodeSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>OpenCode Go</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    push_window_lines(
        &mut lines,
        theme,
        "󰔛",
        "5h window",
        snap.spent_5h,
        snap.limit_5h,
        snap.pct_5h(),
    );
    push_window_lines(
        &mut lines,
        theme,
        "󰃭",
        "Weekly (7d)",
        snap.spent_week,
        snap.limit_week,
        snap.pct_week(),
    );
    push_window_lines(
        &mut lines,
        theme,
        "󰸗",
        "Monthly (30d)",
        snap.spent_month,
        snap.limit_month,
        snap.pct_month(),
    );

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰃬  {tokens} tokens (30d) · top model {model}</span>",
        tokens = escape(&compact_tokens(snap.tokens_month)),
        model = escape(snap.top_model.as_deref().unwrap_or("—"))
    )));

    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

fn push_window_lines(
    lines: &mut Vec<TooltipLine>,
    theme: &Theme,
    icon: &str,
    label: &str,
    spent: f64,
    limit: f64,
    pct: i32,
) {
    let fg = &theme.fg;
    let dim = &theme.dim;
    let color = severity_color(severity_for(pct), theme);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  {icon}  {label}</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   {}",
        progress_bar(pct, color, theme, None)
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span foreground='{dim}'>{spent} of {limit} · </span>\
         <span font_weight='bold' foreground='{color}'>{pct}%</span>",
        spent = escape(&dollars(spent)),
        limit = escape(&dollars(limit)),
    )));
    lines.push(TooltipLine::Body("".into()));
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Opencode(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snap() -> OpencodeSnapshot {
        OpencodeSnapshot {
            provider: "opencode-go".into(),
            spent_5h: 5.16,
            limit_5h: 12.0,
            spent_week: 15.0,
            limit_week: 30.0,
            spent_month: 45.0,
            limit_month: 60.0,
            tokens_month: 389_769_308,
            top_model: Some("deepseek-v4-pro".into()),
        }
    }

    fn sample_outcome(snap: OpencodeSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Opencode(snap),
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
    fn default_render_shows_5h_and_week_pcts() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.text.contains("43% · 50%"), "text: {}", out.text);
    }

    #[test]
    fn tooltip_shows_all_three_windows_and_top_model() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.tooltip.contains("$5.16"), "tooltip: {}", out.tooltip);
        assert!(out.tooltip.contains("$12.00"));
        assert!(out.tooltip.contains("$30.00"));
        assert!(out.tooltip.contains("$60.00"));
        assert!(out.tooltip.contains("deepseek-v4-pro"));
        assert!(out.tooltip.contains("Updated"));
    }

    #[test]
    fn custom_format_uses_specific_placeholders() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let mut o = opts();
        o.format = Some("{oc_month_spent} of {oc_month_limit} ({oc_month_pct}%)".into());
        let out = render(&outcome, &snap, &theme, &o, Utc::now());
        assert!(out.text.contains("$45.00 of $60.00 (75%)"), "{}", out.text);
    }

    #[test]
    fn cross_vendor_aliases_map_to_5h_week_and_month() {
        let values = build_placeholders(&sample_snap());
        assert_eq!(values.get("session_pct").map(String::as_str), Some("43"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("50"));
        // Rolling windows have no reset instant.
        assert_eq!(values.get("session_reset").map(String::as_str), Some("—"));
        assert_eq!(values.get("plan").map(String::as_str), Some("OpenCode Go"));
        // The money-denominated bucket aliases (used by the GNOME extension
        // and macOS menu bar app) map to the monthly window.
        assert_eq!(values.get("extra_pct").map(String::as_str), Some("75"));
        assert_eq!(values.get("extra_spent").map(String::as_str), Some("$45.00"));
        assert_eq!(values.get("extra_limit").map(String::as_str), Some("$60.00"));
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn severity_tracks_worst_window() {
        let mut snap = sample_snap(); // month at 75% → High
        assert_eq!(severity(&snap), PaceSeverity::High);
        snap.spent_month = 60.0; // 100% → Critical
        assert_eq!(severity(&snap), PaceSeverity::Critical);
        let zero = OpencodeSnapshot {
            limit_5h: 12.0,
            limit_week: 30.0,
            limit_month: 60.0,
            ..Default::default()
        };
        assert_eq!(severity(&zero), PaceSeverity::Low);
    }

    #[test]
    fn compact_tokens_scales_units() {
        assert_eq!(compact_tokens(0), "0");
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(1_500), "1.5k");
        assert_eq!(compact_tokens(389_769_308), "389.8M");
        assert_eq!(compact_tokens(2_100_000_000), "2.1B");
    }
}
