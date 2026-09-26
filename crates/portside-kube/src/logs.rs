//! Container log retrieval.

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, LogParams};
use kube::Client;
use portside_core::jiff::Timestamp;

use crate::Result;

pub struct LogRequest<'a> {
    pub namespace: &'a str,
    pub pod: &'a str,
    pub container: &'a str,
    /// Only lines at or after this instant (nanoseconds since epoch).
    pub since_ns: Option<i64>,
    pub since_seconds: Option<i64>,
    pub tail_lines: Option<i64>,
    /// The previous (crashed) instance of the container.
    pub previous: bool,
    pub limit_bytes: Option<i64>,
}

/// Fetch logs with API-server timestamps prefixed on every line.
pub async fn fetch(client: &Client, req: &LogRequest<'_>) -> Result<String> {
    let api: Api<Pod> = Api::namespaced(client.clone(), req.namespace);
    let since_time = req
        .since_ns
        .and_then(|ns| Timestamp::from_nanosecond(ns as i128).ok());
    let lp = LogParams {
        container: Some(req.container.to_string()),
        timestamps: true,
        previous: req.previous,
        since_time,
        since_seconds: if since_time.is_some() { None } else { req.since_seconds },
        tail_lines: req.tail_lines,
        limit_bytes: req.limit_bytes,
        ..Default::default()
    };
    Ok(api.logs(req.pod, &lp).await?)
}
