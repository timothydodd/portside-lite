//! Service port-forwards: a local listener on 127.0.0.1 per forward; every
//! accepted connection is piped to a Ready pod behind the Service through
//! the API server. Pods are re-resolved when the current one fails, so a
//! forward survives rollouts and restarts (unlike `kubectl port-forward`).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use portside_core::now_ms;
use portside_kube::portforward::{self, Target};
use portside_kube::ClusterClient;
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};

use crate::Monitor;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    pub id: u64,
    pub profile_id: String,
    pub cluster_name: String,
    pub namespace: String,
    pub service: String,
    pub service_port: i32,
    pub local_port: u16,
    /// Pod currently receiving connections, once one has been resolved.
    pub pod: Option<String>,
    pub target_port: Option<u16>,
    pub active_connections: usize,
    pub total_connections: u64,
    pub last_error: Option<String>,
    pub started_ms: i64,
}

pub(crate) struct ForwardEntry {
    info: Arc<StdMutex<ForwardInfo>>,
    shutdown: watch::Sender<bool>,
}

/// Shared by one forward's connections: the client to use and where to send.
struct Route {
    client: Arc<ClusterClient>,
    target: Option<Target>,
}

impl Monitor {
    pub fn list_forwards(&self) -> Vec<ForwardInfo> {
        let mut v: Vec<ForwardInfo> =
            self.forwards.lock().unwrap().values().map(|e| e.info.lock().unwrap().clone()).collect();
        v.sort_by_key(|f| f.id);
        v
    }

    fn emit_forwards(&self) {
        self.sink.forwards_changed(&self.list_forwards());
    }

    /// Forward `service:service_port` in the active cluster to 127.0.0.1.
    /// `local_port: None` picks a memorable free port (80→8080, …).
    pub async fn start_forward(
        self: &Arc<Self>,
        namespace: &str,
        service: &str,
        service_port: i32,
        local_port: Option<u16>,
    ) -> Result<ForwardInfo, String> {
        let (profile_id, cluster_name) = {
            let s = self.settings.read().unwrap();
            let p = s.active_profile().ok_or("No cluster connection configured.")?;
            (p.id.clone(), p.name.clone())
        };
        if let Some(existing) = self.list_forwards().into_iter().find(|f| {
            f.profile_id == profile_id && f.namespace == namespace && f.service == service && f.service_port == service_port
        }) {
            return Err(format!("{service}:{service_port} is already forwarded to 127.0.0.1:{}", existing.local_port));
        }

        let client = self.client().await?;
        // Fail fast (no Ready pod, unknown port) before opening a local port.
        let target = portforward::resolve(&client.client, namespace, service, service_port, None)
            .await
            .map_err(|e| e.to_string())?;

        let listener = match local_port {
            Some(p) => TcpListener::bind(("127.0.0.1", p))
                .await
                .map_err(|e| format!("Local port {p} isn't available ({e}). Pick another."))?,
            None => {
                let preferred = portforward::suggest_local_port(service_port);
                match TcpListener::bind(("127.0.0.1", preferred)).await {
                    Ok(l) => l,
                    Err(_) => TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| e.to_string())?,
                }
            }
        };
        let bound = listener.local_addr().map_err(|e| e.to_string())?.port();

        let id = self.next_forward.fetch_add(1, Ordering::Relaxed);
        let info = Arc::new(StdMutex::new(ForwardInfo {
            id,
            profile_id: profile_id.clone(),
            cluster_name,
            namespace: namespace.into(),
            service: service.into(),
            service_port,
            local_port: bound,
            pod: Some(target.pod.clone()),
            target_port: Some(target.port),
            active_connections: 0,
            total_connections: 0,
            last_error: None,
            started_ms: now_ms(),
        }));
        let (shutdown, shutdown_rx) = watch::channel(false);
        let route = Arc::new(Mutex::new(Route { client, target: Some(target) }));

        let monitor = Arc::clone(self);
        let info2 = Arc::clone(&info);
        let (ns, svc) = (namespace.to_string(), service.to_string());
        tokio::spawn(async move {
            let mut stop = shutdown_rx.clone();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else {
                            // e.g. out of file descriptors: don't spin on it.
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            continue;
                        };
                        let _ = socket.set_nodelay(true);
                        tokio::spawn(serve_connection(
                            Arc::clone(&monitor), Arc::clone(&info2), Arc::clone(&route),
                            profile_id.clone(), ns.clone(), svc.clone(), service_port, socket, shutdown_rx.clone(),
                        ));
                    }
                    _ = stop.changed() => break, // listener drops here, freeing the port
                }
            }
        });

        let snapshot = info.lock().unwrap().clone();
        {
            // Checked again under the lock: two starts for the same port can both pass the check above.
            let mut forwards = self.forwards.lock().unwrap();
            let twin = forwards.values().map(|e| e.info.lock().unwrap().clone()).find(|f| {
                f.profile_id == snapshot.profile_id && f.namespace == namespace && f.service == service && f.service_port == service_port
            });
            if let Some(twin) = twin {
                let _ = shutdown.send(true);
                return Err(format!("{service}:{service_port} is already forwarded to 127.0.0.1:{}", twin.local_port));
            }
            forwards.insert(id, ForwardEntry { info, shutdown });
        }
        self.emit_forwards();
        Ok(snapshot)
    }

    /// Stop forwards whose profile was deleted or now points somewhere else:
    /// their next reconnect would look the Service up on a different cluster.
    pub(crate) fn stop_orphaned_forwards(&self, old: &portside_core::Settings, new: &portside_core::Settings) {
        let find = |s: &portside_core::Settings, id: &str| s.connections.iter().find(|p| p.id == id).map(|p| p.connection.clone());
        let orphaned: Vec<u64> = self
            .list_forwards()
            .into_iter()
            .filter(|f| match (find(old, &f.profile_id), find(new, &f.profile_id)) {
                (Some(was), Some(is)) => !was.same_endpoint(&is),
                (_, None) => true,
                (None, Some(_)) => false,
            })
            .map(|f| f.id)
            .collect();
        for id in orphaned {
            self.stop_forward(id);
        }
    }

    /// Close the local port and every open connection of a forward.
    pub fn stop_forward(&self, id: u64) {
        if let Some(entry) = self.forwards.lock().unwrap().remove(&id) {
            let _ = entry.shutdown.send(true);
        }
        self.emit_forwards();
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    monitor: Arc<Monitor>,
    info: Arc<StdMutex<ForwardInfo>>,
    route: Arc<Mutex<Route>>,
    profile_id: String,
    namespace: String,
    service: String,
    service_port: i32,
    mut socket: TcpStream,
    mut shutdown: watch::Receiver<bool>,
) {
    {
        let mut i = info.lock().unwrap();
        i.active_connections += 1;
        i.total_connections += 1;
    }
    monitor.emit_forwards();

    // Stopping the forward also gives up on a connection that's still being set up.
    let connected = tokio::select! {
        c = connect(&monitor, &route, &profile_id, &namespace, &service, service_port) => c,
        _ = shutdown.wait_for(|stop| *stop) => Err("Stopped.".to_string()),
    };
    match connected {
        Ok((pf, mut stream, target)) => {
            {
                let mut i = info.lock().unwrap();
                i.pod = Some(target.pod);
                i.target_port = Some(target.port);
                i.last_error = None;
            }
            monitor.emit_forwards();
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut socket, &mut stream) => {}
                _ = shutdown.changed() => {}
            }
            drop(stream);
            // The pod-side failure (e.g. nothing listening on the port) arrives here.
            if let Ok(Err(e)) = tokio::time::timeout(Duration::from_secs(5), pf.join()).await {
                info.lock().unwrap().last_error = Some(e.to_string());
            }
        }
        Err(e) => {
            info.lock().unwrap().last_error = Some(e);
        }
    }

    info.lock().unwrap().active_connections -= 1;
    monitor.emit_forwards();
}

/// Open a stream to the current pod; if that fails, re-resolve (the pod may
/// have gone) with a fresh client (the connection may have dropped) and try once more.
async fn connect(
    monitor: &Arc<Monitor>,
    route: &Arc<Mutex<Route>>,
    profile_id: &str,
    namespace: &str,
    service: &str,
    service_port: i32,
) -> Result<(portforward::Portforwarder, impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin, Target), String> {
    let (client, target) = {
        let r = route.lock().await;
        (Arc::clone(&r.client), r.target.clone())
    };
    if let Some(t) = target {
        if let Ok((pf, stream)) = portforward::open(&client.client, namespace, &t).await {
            return Ok((pf, stream, t));
        }
    }

    // Slow path: fresh client + fresh pod.
    let client = monitor.connect_profile(profile_id).await?;
    let current = route.lock().await.target.as_ref().map(|t| t.pod.clone());
    let t = portforward::resolve(&client.client, namespace, service, service_port, current.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    let opened = portforward::open(&client.client, namespace, &t).await.map_err(|e| e.to_string());
    {
        let mut r = route.lock().await;
        r.client = client;
        r.target = Some(t.clone());
    }
    let (pf, stream) = opened?;
    Ok((pf, stream, t))
}
