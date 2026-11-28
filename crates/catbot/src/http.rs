//! Shared HTTP clients for outbound requests, keeping TLS roots and connection pools alive
//! instead of rebuilding a fresh `reqwest::Client` on every request.

use std::sync::OnceLock;
use std::time::Duration;

/// Shared pooled HTTP client for standard JSON/API requests. Callers attach per-request
/// `.timeout(...)` on the `RequestBuilder` when needed.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(8)
            .user_agent(concat!("Catbot/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Shared pooled HTTP client for streaming responses (SSE, model pulls, release downloads),
/// enforcing a connect timeout and a per-chunk read idle timeout so stalled streams cannot hang
/// tasks forever.
pub fn stream_client() -> &'static reqwest::Client {
    static STREAM: OnceLock<reqwest::Client> = OnceLock::new();
    STREAM.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .user_agent(concat!("Catbot/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}
