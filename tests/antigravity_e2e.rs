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

// Wire-faithful body captured from a live language server (values swapped
// for stable snapshot percentages): {"response": …} envelope, fraction
// directly on the bucket, explicit "window" discriminator.
const SUMMARY_BODY: &str = r#"{"response":{"groups":[
    {"displayName":"Gemini Models","buckets":[
        {"bucketId":"gemini-weekly","displayName":"Weekly Limit",
         "window":"weekly","remainingFraction":0.72,
         "resetTime":"2026-07-05T00:00:00Z"},
        {"bucketId":"gemini-5h","displayName":"Five Hour Limit",
         "window":"5h","remainingFraction":0.37,
         "resetTime":"2026-07-01T14:30:00Z"}
    ]},
    {"displayName":"Claude and GPT models","buckets":[
        {"bucketId":"3p-weekly","displayName":"Weekly Limit",
         "window":"weekly","remainingFraction":0.55,
         "resetTime":"2026-07-05T00:00:00Z"},
        {"bucketId":"3p-5h","displayName":"Five Hour Limit",
         "window":"5h","remainingFraction":0.10,
         "resetTime":"2026-07-01T14:30:00Z"}
    ]}
]}}"#;

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
