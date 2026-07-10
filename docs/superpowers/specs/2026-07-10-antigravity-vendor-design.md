# Antigravity vendor — design spec

Date: 2026-07-10
Status: approved (brainstorming session)

## Goal

Add a new vendor `antigravity` that surfaces Google Antigravity IDE plan
consumption (quota buckets) in every existing surface — Waybar widget, TUI
panel, and the macOS menu bar app — following the local-first precedent set
by the `opencode` vendor.

## Background

Google Antigravity (desktop IDE, VS Code fork) enforces plan quotas in two
groups — **Gemini Models** and **Claude and GPT models** — each with a
**5-hour session** window and a **weekly** window (4 buckets total). The IDE
exposes these numbers through its local language server via Connect-RPC over
loopback HTTPS. There is no documented public API; the mechanics below are
the same ones CodexBar uses in production (see
`steipete/CodexBar` `docs/antigravity.md`).

Verified on this machine:

- IDE installed at `/Applications/Antigravity IDE.app`; CLI `agy` is a
  symlink into the app bundle.
- The language server process carries `--csrf_token` (and optionally
  `--extension_server_port` / `--extension_server_csrf_token`) on its
  command line.
- `state.vscdb` holds a Codeium-style `apiKey` but no readable OAuth token —
  which is why the remote (`cloudcode-pa.googleapis.com`) route was rejected
  in favor of the local probe.

## Decisions made

| Decision | Choice |
|---|---|
| Product | Antigravity IDE (not Gemini CLI) |
| Fetch strategy | **A — local language-server probe** (OAuth remote and hybrid rejected: large lift, ToS-gray client-secret extraction, encrypted token storage) |
| Bar mapping | primary = Gemini 5h, secondary = Gemini weekly, extra = Claude+GPT 5h; Claude+GPT weekly tooltip/TUI-only |
| IDE closed | serve stale cache (existing mechanism); no cache → ⚠ fallback, exit 0 |

## Architecture

New module `src/antigravity/{mod.rs, fetch.rs, types.rs, vendor.rs}`,
mirroring `src/opencode/`.

### Fetch pipeline (`fetch.rs`)

1. **Process discovery** — run `ps -ax -o pid=,command=`; classify lines as
   Antigravity language servers. Markers: command line contains a language
   server binary AND an antigravity path/`--app_data_dir antigravity`
   marker. Two classes:
   - IDE/app language server — requires `--csrf_token`; a tokenless match is
     skipped (never probed without token).
   - CLI (`agy` / `antigravity-cli`) language server — no CSRF required.
   Extract `--csrf_token`, and `--extension_server_port` +
   `--extension_server_csrf_token` when present (direct port shortcut).
2. **Port discovery** — `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` for each
   candidate pid.
3. **Port validation** — `POST
   https://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/GetUnleashData`
   with headers `X-Codeium-Csrf-Token: <token>` and
   `Connect-Protocol-Version: 1`. First 200 wins. When both IDE and CLI are
   up, first responding endpoint wins.
4. **Quota fetch** — `POST …/RetrieveUserQuotaSummary` (preferred), fallback
   `POST …/GetUserStatus` (legacy shape).
5. **TLS** — the local server uses a self-signed cert. The reqwest client
   for this vendor sets `danger_accept_invalid_certs(true)` and is used
   **only** against `127.0.0.1`. No other vendor shares this client.
6. **Failure ladder** — no process / no port / all probes fail → read
   per-vendor cache and mark stale; no cache → widget fallback ⚠ JSON,
   exit 0 (hard invariant).

### Types (`types.rs`)

Serde structs (camelCase) for the `RetrieveUserQuotaSummary` response:

```
response.groups[].displayName
response.groups[].buckets[].bucketId
response.groups[].buckets[].displayName
response.groups[].buckets[].remaining.remainingFraction
response.groups[].buckets[].description
```

plus the `GetUserStatus` legacy fallback
(`userStatus.cascadeModelConfigData.clientModelConfigs[].quotaInfo.{remainingFraction,resetTime}`).
Reset times parse as ISO-8601 or epoch seconds. Group → bucket mapping is by
group `displayName` ("Gemini" vs "Claude"/"GPT" substring match) and window
inferred from `bucketId` / display name (5h vs weekly).

### Snapshot (`usage.rs`)

New `VendorSnapshot::Antigravity` variant with four optional buckets:
`gemini_session`, `gemini_weekly`, `claude_gpt_session`, `claude_gpt_weekly`.
Each bucket: `used_pct = 1 − remainingFraction` (clamped 0..=100) and an
optional reset timestamp. A bucket missing `remainingFraction` is stored as
`None` — renders as `—`, never panics.

Cross-vendor placeholders `extra_pct` / `extra_spent` / `extra_limit` are
filled from the Claude+GPT session bucket so the GNOME extension and the
macOS menu bar render a third bar with no vendor-specific wiring (same trick
as opencode's monthly window).

## Surfaces

- `src/vendor.rs` — `VendorId::Antigravity`, slug `"antigravity"`, added to
  `all()`.
- `src/active.rs` — scroll-cycle order.
- `src/widget/cli.rs`, `src/widget/run.rs` — CLI value + fetch/render wiring.
- `src/tui/panels.rs` — native panel with four gauges (both groups, both
  windows). `src/tui/app.rs`, `src/tui/view.rs`, `src/tui/settings.rs` —
  registration.
- `macos/ai-usagebar-menubar.swift` — Preferences picker + Vendors section
  entry ("configured" detection = Antigravity IDE app present or language
  server reachable; no login flow). Row labels: Session, Weekly, and the
  extra row labeled for Claude+GPT 5h.
- `config.example.toml` — minimal `[antigravity]` section (no plan limits to
  configure; the API returns fractions). Refresh cadence follows existing
  global knobs.
- `README.md` + `CHANGELOG.md` entries.

## Testing

Hermetic (repo hard invariant — no real `$HOME`, no ambient env):

- **Pure parsers, injectable inputs**: `process_infos_from_ps_output(&str)`
  (classification, CSRF/port flag extraction — fixture strings incl.
  tokenless-IDE and CLI cases), lsof output → ports, reset-time parsing
  (ISO-8601 + epoch), quota-summary → snapshot mapping.
- **E2E**: `tests/antigravity_e2e.rs` with mockito + insta snapshots
  (mold: `tests/opencode_e2e.rs`). Fetch takes an injected base URL; mocks
  run over `http://`, production builds `https://127.0.0.1:<port>`.
- **Live smoke**: `#[ignore]`d test in `tests/live.rs` probing the real IDE.
- Gate: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo machete`.

## Risks

- **Undocumented surface** — an Antigravity update can rename endpoints or
  reshape the response. Mitigation: `make smoke` discipline (capture new
  response, fix `types.rs`, bump `pkgrel` only).
- **Self-signed TLS exception** — scoped to a dedicated loopback-only
  client; never applied to remote hosts.
- **IDE not running** — expected steady-state for a menu bar widget;
  answered by stale cache, not an error.

## Out of scope (YAGNI)

- Google OAuth remote fetch (`cloudcode-pa.googleapis.com`) and hybrid
  fallback — possible later without breaking this design.
- Spawning `agy` in a PTY to force-start a quota server.
- Gemini CLI (`~/.gemini/oauth_creds.json`) as a vendor — separate product,
  separate future spec.
