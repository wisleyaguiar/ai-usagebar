//! Discovery + fetch for the Antigravity language server.
//!
//! Discovery is two subprocess calls (`ps`, `lsof`) whose OUTPUT PARSING is
//! pure and unit-tested; the subprocess execution itself is a thin shell.
//! The quota fetch is Connect-RPC-over-HTTPS JSON POSTs against loopback.
//! The IDE's server authenticates local requests with the `--csrf_token`
//! from its own command line; the CLI (`agy`) server needs no token.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::AntigravitySnapshot;

use super::types::{QuotaSummaryResponse, UserStatusResponse, parse_cache, snap_to_json};

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const LS_SERVICE: &str = "/exa.language_server_pb.LanguageServerService";

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
    bin.ends_with("/agy")
        || bin == "agy"
        || cmd_lower.contains("antigravity-cli")
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
    // is still starting. Keep the cache, note the error.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
        let line =
            "60 /apps/antigravity/language_server --csrf_token=tok-123 --extension_server_port=4242";
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
        assert_eq!(
            out.snapshot.claude_gpt_session.as_ref().unwrap().utilization_pct,
            90
        );
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
            Endpoint {
                base_url: "http://127.0.0.1:1".into(),
                csrf_token: None,
            },
            Endpoint {
                base_url: server.url(),
                csrf_token: None,
            },
        ];
        let out = fetch_snapshot(&client, &endpoints, &cache, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(out.snapshot.gemini_session.as_ref().unwrap().utilization_pct, 63);
    }
}
