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
    /// The cursor (nanoseconds since epoch). The server only takes whole
    /// seconds, so lines from the start of this instant's second come back;
    /// the caller drops the ones at or before the cursor.
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
    let since_time = req.since_ns.and_then(since_time);
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

/// `sinceTime` for a cursor, floored to its second. kube sends whole seconds
/// and rounds half up, so an unfloored cursor at `…26.7` would ask for `…27`
/// and the lines in between would never be fetched.
fn since_time(cursor_ns: i64) -> Option<Timestamp> {
    Timestamp::from_second(cursor_ns.div_euclid(1_000_000_000)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_time_never_moves_past_the_cursor() {
        for (cursor_ns, second) in [(26_700_000_000, 26), (26_000_000_000, 26), (26_999_999_999, 26), (-300_000_000, -1)] {
            let t = since_time(cursor_ns).unwrap();
            assert_eq!((t.as_second(), t.subsec_nanosecond()), (second, 0), "cursor {cursor_ns}");
        }
    }
}
