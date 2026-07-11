//! Google Antigravity vendor — a local-first vendor that probes the
//! Antigravity IDE's language server over loopback HTTPS (Connect-RPC) for
//! plan quota buckets. No credentials leave the machine; the CSRF token and
//! port come from the running process itself. When the IDE isn't running,
//! the cached snapshot is served (marked stale).

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{Endpoint, FetchOutcome, discover_endpoints, fetch_snapshot, loopback_client};
