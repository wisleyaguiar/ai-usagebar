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
