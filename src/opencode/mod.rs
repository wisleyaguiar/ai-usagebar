//! OpenCode Go vendor — the first local-first vendor: instead of calling an
//! HTTP API, it aggregates per-message spend from the opencode CLI's local
//! SQLite database (`~/.local/share/opencode/opencode.db`) and measures it
//! against the Go plan's dollar limits ($12/5h, $30/week, $60/month —
//! configurable, since OpenCode doesn't expose them via any API).

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, default_db_path, fetch_snapshot};
