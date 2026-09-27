//! Port-forwarding to a Service: pick a Ready pod behind it, map the Service
//! port to the container port (named ports included), and open a forwarded
//! stream through the API server (works over the SSH tunnel too).

use k8s_openapi::api::core::v1::{Pod, Service, ServicePort};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, ListParams};
pub use kube::api::Portforwarder;
use kube::Client;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{KubeError, Result};

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// Where connections for a Service port currently go.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub pod: String,
    pub port: u16,
}

fn is_ready(p: &Pod) -> bool {
    p.metadata.deletion_timestamp.is_none()
        && p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
        && p.status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}

/// A Ready pod to forward to, preferring the one already in use (sticky, so
/// long-lived connections keep landing on the same pod), then the newest.
pub fn pick_pod<'a>(pods: &'a [Pod], current: Option<&str>) -> Option<&'a Pod> {
    let ready: Vec<&Pod> = pods.iter().filter(|p| is_ready(p)).collect();
    if let Some(cur) = current {
        if let Some(p) = ready.iter().find(|p| p.metadata.name.as_deref() == Some(cur)) {
            return Some(p);
        }
    }
    ready.into_iter().max_by_key(|p| p.metadata.creation_timestamp.as_ref().map(|t| t.0))
}

/// The container port a Service port sends traffic to on `pod`: the numeric
/// targetPort, a named port looked up in the pod's containers, or (no
/// targetPort) the Service port itself.
pub fn target_port(svc_port: &ServicePort, pod: &Pod) -> Option<u16> {
    match &svc_port.target_port {
        None => u16::try_from(svc_port.port).ok(),
        Some(IntOrString::Int(n)) => u16::try_from(*n).ok(),
        Some(IntOrString::String(name)) => pod
            .spec
            .as_ref()?
            .containers
            .iter()
            .flat_map(|c| c.ports.iter().flatten())
            .find(|p| p.name.as_deref() == Some(name.as_str()))
            .and_then(|p| u16::try_from(p.container_port).ok()),
    }
}

/// Resolve a Service port to a concrete pod + container port right now.
pub async fn resolve(client: &Client, namespace: &str, service: &str, service_port: i32, current_pod: Option<&str>) -> Result<Target> {
    let svc = Api::<Service>::namespaced(client.clone(), namespace).get(service).await?;
    let spec = svc.spec.unwrap_or_default();
    let port = spec
        .ports
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.port == service_port)
        .ok_or_else(|| other(format!("Service {service} has no port {service_port}")))?;
    let selector = spec.selector.unwrap_or_default();
    if selector.is_empty() {
        return Err(other(format!("Service {service} has no selector, so there's no pod to forward to")));
    }
    let labels = selector.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
    let pods = Api::<Pod>::namespaced(client.clone(), namespace)
        .list(&ListParams::default().labels(&labels))
        .await?
        .items;
    let pod = pick_pod(&pods, current_pod)
        .ok_or_else(|| other(format!("No Ready pod behind Service {service} (selector {labels})")))?;
    let port_no = target_port(&port, pod)
        .ok_or_else(|| other(format!("Couldn't map Service port {service_port} to a container port")))?;
    Ok(Target { pod: pod.metadata.name.clone().unwrap_or_default(), port: port_no })
}

/// Open one forwarded byte stream to `pod:port`. Keep the returned
/// `Portforwarder` alive for as long as the stream is used, then `join` it.
pub async fn open(client: &Client, namespace: &str, target: &Target) -> Result<(Portforwarder, impl AsyncRead + AsyncWrite + Unpin + use<>)> {
    let mut pf = Api::<Pod>::namespaced(client.clone(), namespace).portforward(&target.pod, &[target.port]).await?;
    let stream = pf.take_stream(target.port).ok_or_else(|| other("port-forward stream unavailable"))?;
    Ok((pf, stream))
}

/// A sensible local port for a Service port: the same number when it's
/// unprivileged, otherwise a memorable high port (80→8080, 443→8443,
/// 22→10022).
pub fn suggest_local_port(service_port: i32) -> u16 {
    match service_port {
        80 => 8080,
        443 => 8443,
        p if (1024..=65535).contains(&p) => p as u16,
        p if (1..1024).contains(&p) => (10000 + p) as u16,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(name: &str, ready: bool, created: &str, named_port: Option<(&str, i32)>) -> Pod {
        let ports = named_port.map(|(n, p)| serde_json::json!([{ "name": n, "containerPort": p }])).unwrap_or(serde_json::json!([]));
        serde_json::from_value(serde_json::json!({
            "metadata": { "name": name, "creationTimestamp": created },
            "spec": { "containers": [{ "name": "app", "ports": ports }] },
            "status": { "phase": "Running", "conditions": [{ "type": "Ready", "status": if ready { "True" } else { "False" } }] }
        }))
        .unwrap()
    }

    #[test]
    fn picks_ready_sticky_then_newest() {
        let pods = vec![
            pod("old", true, "2026-01-01T00:00:00Z", None),
            pod("new", true, "2026-06-01T00:00:00Z", None),
            pod("broken", false, "2026-09-01T00:00:00Z", None),
        ];
        assert_eq!(pick_pod(&pods, None).unwrap().metadata.name.as_deref(), Some("new"));
        assert_eq!(pick_pod(&pods, Some("old")).unwrap().metadata.name.as_deref(), Some("old"), "sticky");
        assert_eq!(pick_pod(&pods, Some("broken")).unwrap().metadata.name.as_deref(), Some("new"), "never an unready pod");
        assert!(pick_pod(&[pod("x", false, "2026-01-01T00:00:00Z", None)], None).is_none());
    }

    #[test]
    fn maps_numeric_named_and_default_target_ports() {
        let p = pod("p", true, "2026-01-01T00:00:00Z", Some(("http", 8080)));
        let sp = |target: Option<IntOrString>| ServicePort { port: 80, target_port: target, ..Default::default() };
        assert_eq!(target_port(&sp(None), &p), Some(80));
        assert_eq!(target_port(&sp(Some(IntOrString::Int(9000))), &p), Some(9000));
        assert_eq!(target_port(&sp(Some(IntOrString::String("http".into()))), &p), Some(8080));
        assert_eq!(target_port(&sp(Some(IntOrString::String("nope".into()))), &p), None);
    }

    #[test]
    fn local_port_suggestions() {
        assert_eq!(suggest_local_port(80), 8080);
        assert_eq!(suggest_local_port(443), 8443);
        assert_eq!(suggest_local_port(5432), 5432);
        assert_eq!(suggest_local_port(22), 10022);
    }
}
