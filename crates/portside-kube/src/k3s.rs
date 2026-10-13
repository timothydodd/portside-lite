//! See whether an SSH host has k3s, and install it there. The commands
//! come from `portside_core::k3s`; the installer's output is streamed to
//! the caller line by line.

use std::time::Duration;

use portside_core::k3s::{self as core_k3s, HostProbe, K3sInstallOptions};
use portside_core::SshConnection;

use crate::ssh::Session;
use crate::{sudo_command, KubeError, Result};

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// How long the node gets to become Ready after the installer finishes.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// Upper bound on an OS update plus the install.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(40 * 60);

/// Connect and see what's on the host.
pub async fn probe(cfg: &SshConnection) -> Result<HostProbe> {
    let session = Session::connect(cfg).await?;
    let out = session.exec(core_k3s::PROBE_SCRIPT, None).await?;
    let mut probe = core_k3s::parse_probe(&out);
    let password = cfg.effective_sudo_password();
    probe.sudo_ok = session.exec(&sudo_command("sudo -n true", password.is_some()), password).await.is_ok();
    probe.host_key_fingerprint = Some(session.fingerprint.clone());
    Ok(probe)
}

/// Install k3s: optionally update the OS first, make sure curl is there,
/// run the official installer with the chosen flags, then wait for the node
/// to be Ready. Every line of output goes to `log`.
pub async fn install(cfg: &SshConnection, opts: &K3sInstallOptions, log: &mut (dyn FnMut(&str) + Send)) -> Result<HostProbe> {
    core_k3s::validate(opts).map_err(other)?;
    tokio::time::timeout(INSTALL_TIMEOUT, install_inner(cfg, opts, log))
        .await
        .map_err(|_| other(format!("Gave up after {} minutes; the server may still be installing.", INSTALL_TIMEOUT.as_secs() / 60)))?
}

async fn install_inner(cfg: &SshConnection, opts: &K3sInstallOptions, log: &mut (dyn FnMut(&str) + Send)) -> Result<HostProbe> {
    let session = Session::connect(cfg).await?;
    let password = cfg.effective_sudo_password();
    let out = session.exec(core_k3s::PROBE_SCRIPT, None).await?;
    let before = core_k3s::parse_probe(&out);
    if before.k3s_installed {
        return Err(other(format!("k3s is already installed on {} ({}).", cfg.host, before.k3s_version.unwrap_or_default())));
    }
    if !before.has_systemd {
        return Err(other("k3s needs systemd (or openrc) to run as a service, and this host has neither systemctl nor a known init."));
    }

    let pm = before.package_manager.as_deref();
    if opts.update_system {
        match pm.and_then(core_k3s::update_command) {
            Some(cmd) => run(&session, password, log, "Update the system packages", &cmd).await?,
            None => log("No known package manager (apt, dnf, yum, zypper, apk); skipping the system update.\n"),
        }
    } else if !before.has_curl {
        match pm.and_then(core_k3s::curl_command) {
            Some(cmd) => run(&session, password, log, "Install curl", &cmd).await?,
            None => return Err(other("curl isn't installed and there's no known package manager to install it with.")),
        }
    }

    run(&session, password, log, "Install k3s", &core_k3s::install_command(opts)).await?;

    log("Waiting for the node to be Ready…\n");
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        let out = session.exec(&sudo_command(core_k3s::READY_COMMAND, password.is_some()), password).await.unwrap_or_default();
        if core_k3s::node_ready(&out) {
            log(&format!("{out}\n"));
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(other(format!(
                "k3s was installed, but the node wasn't Ready after {} s. Check `journalctl -u k3s` on the server.",
                READY_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let out = session.exec(core_k3s::PROBE_SCRIPT, None).await?;
    let mut after = core_k3s::parse_probe(&out);
    after.sudo_ok = true;
    after.host_key_fingerprint = Some(session.fingerprint.clone());
    Ok(after)
}

/// Run one step, echoing the command's label and its output to the log;
/// a non-zero exit is an error.
async fn run(session: &Session, password: Option<&str>, log: &mut (dyn FnMut(&str) + Send), label: &str, command: &str) -> Result<()> {
    log(&format!("$ {label}\n"));
    let code = session.exec_stream(&sudo_command(command, password.is_some()), password, log).await?;
    if code != 0 {
        return Err(other(format!("{label} exited with status {code}; see the output above.")));
    }
    Ok(())
}
