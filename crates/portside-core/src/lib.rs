//! Pure domain logic for Portside Lite: settings, the snapshot DTOs the UI
//! renders, Kubernetes quantity parsing, log-line parsing and the issue
//! detection rules. No IO, no Tauri — everything here is unit-testable.

pub mod alerts;
pub mod issues;
pub mod logline;
pub mod manifest;
pub mod models;
pub mod quantity;
pub mod settings;
pub mod summarize;

pub use models::*;
pub use settings::*;

/// Re-export so downstream crates share one timestamp type with k8s-openapi.
pub use k8s_openapi::jiff;

/// Milliseconds since the Unix epoch for "now".
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
