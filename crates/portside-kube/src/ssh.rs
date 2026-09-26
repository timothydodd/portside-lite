//! SSH session with trust-on-first-use host key pinning, remote command
//! execution, and a local TCP listener forwarded to the API server through
//! `direct-tcpip` channels.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use portside_core::{SshAuth, SshConnection};
use russh::client::{self, Handle};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::{KubeError, Result};

fn err(e: impl std::fmt::Display) -> KubeError {
    KubeError::Ssh(e.to_string())
}

struct Verifier {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let public = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => cert.public_key().clone().into(),
        };
        let fp = public.fingerprint(HashAlg::Sha256).to_string();
        *self.seen.lock().unwrap() = Some(fp.clone());
        Ok(match &self.expected {
            Some(expected) => *expected == fp,
            None => true,
        })
    }
}

pub struct Session {
    handle: Arc<Handle<Verifier>>,
    pub fingerprint: String,
}

impl Session {
    pub async fn connect(cfg: &SshConnection) -> Result<Session> {
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            inactivity_timeout: None,
            ..Default::default()
        });
        let seen = Arc::new(Mutex::new(None));
        let verifier = Verifier {
            expected: cfg.host_key_fingerprint.clone().filter(|f| !f.is_empty()),
            seen: Arc::clone(&seen),
        };

        let connecting = client::connect(config, (cfg.host.as_str(), cfg.port), verifier);
        let result = tokio::time::timeout(Duration::from_secs(15), connecting)
            .await
            .map_err(|_| err(format!("timed out connecting to {}:{}", cfg.host, cfg.port)))?;
        let mut handle = match result {
            Ok(h) => h,
            Err(e) => {
                let presented = seen.lock().unwrap().clone();
                if let (Some(expected), Some(presented)) = (&cfg.host_key_fingerprint, presented) {
                    if *expected != presented {
                        return Err(err(format!(
                            "HOST KEY CHANGED for {} — expected {expected}, got {presented}. \
                             If the server was rebuilt, clear the pinned fingerprint in Settings.",
                            cfg.host
                        )));
                    }
                }
                return Err(err(e));
            }
        };

        let auth = match &cfg.auth {
            SshAuth::Password { password } => handle
                .authenticate_password(&cfg.username, password)
                .await
                .map_err(err)?,
            SshAuth::Key { private_key_path, passphrase } => {
                let path = expand_home(private_key_path);
                let key = keys::load_secret_key(&path, passphrase.as_deref().filter(|p| !p.is_empty()))
                    .map_err(|e| err(format!("can't load key {path}: {e}")))?;
                let hash = handle.best_supported_rsa_hash().await.map_err(err)?.flatten();
                handle
                    .authenticate_publickey(&cfg.username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await
                    .map_err(err)?
            }
        };
        if !auth.success() {
            return Err(err(format!("authentication failed for {}@{}", cfg.username, cfg.host)));
        }

        let fingerprint = seen.lock().unwrap().clone().unwrap_or_default();
        Ok(Session { handle: Arc::new(handle), fingerprint })
    }

    /// Run a command and return stdout; non-zero exit is an error carrying
    /// stderr. `stdin` (e.g. a sudo password) is written then closed, so it
    /// never appears on the remote command line.
    pub async fn exec(&self, command: &str, stdin: Option<&str>) -> Result<String> {
        let mut channel = self.handle.channel_open_session().await.map_err(err)?;
        channel.exec(true, command).await.map_err(err)?;
        if let Some(input) = stdin {
            channel.data(format!("{input}\n").as_bytes()).await.map_err(err)?;
            channel.eof().await.map_err(err)?;
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut code = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                _ => {}
            }
        }
        match code {
            Some(0) | None => Ok(String::from_utf8_lossy(&stdout).into_owned()),
            Some(c) => Err(err(format!(
                "`{command}` exited with {c}: {}",
                String::from_utf8_lossy(&stderr).trim()
            ))),
        }
    }

    /// Listen on 127.0.0.1:<random> and forward each connection to
    /// `remote_host:remote_port` as seen from the SSH server.
    pub async fn tunnel(self, remote_host: String, remote_port: u16) -> Result<Tunnel> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(err)?;
        let local_port = listener.local_addr().map_err(err)?.port();
        let handle = Arc::clone(&self.handle);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, peer)) = listener.accept().await else { break };
                let handle = Arc::clone(&handle);
                let host = remote_host.clone();
                tokio::spawn(async move {
                    let channel = match handle
                        .channel_open_direct_tcpip(host, remote_port as u32, peer.ip().to_string(), peer.port() as u32)
                        .await
                    {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                });
            }
        });
        Ok(Tunnel { local_port, task, _session: self })
    }
}

/// Keeps the SSH session and forwarding task alive; dropping it tears both down.
pub struct Tunnel {
    pub local_port: u16,
    task: JoinHandle<()>,
    _session: Session,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        return format!("{home}/{rest}");
    }
    path.to_string()
}
