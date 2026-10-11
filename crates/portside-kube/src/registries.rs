//! Registry credentials: create a pull secret and hand it to a
//! ServiceAccount, so its pods can pull from a private registry.

use k8s_openapi::api::core::v1::{LocalObjectReference, Secret, ServiceAccount};
use kube::api::{Api, PostParams};
use kube::Client;
use portside_core::registry::{pull_secret, validate_credential, RegistryCredentialInput};

use crate::{KubeError, Result};

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

fn post_params(dry_run: bool) -> PostParams {
    PostParams { dry_run, field_manager: Some("portside-lite".into()) }
}

/// Create the pull secret (never overwrites one that exists), then add it to
/// `service_account`'s `imagePullSecrets` if one is given. Returns what was done.
pub async fn add_credential(
    client: &Client,
    input: &RegistryCredentialInput,
    service_account: Option<&str>,
    dry_run: bool,
) -> Result<Vec<String>> {
    validate_credential(input).map_err(other)?;
    let ns = input.namespace.as_str();
    let secrets: Api<Secret> = Api::namespaced(client.clone(), ns);
    if secrets.get_opt(&input.name).await?.is_some() {
        return Err(other(format!(
            "A Secret named {} already exists in {ns}. Pick another name, or edit that one from the Secrets tab.",
            input.name
        )));
    }
    let accounts: Api<ServiceAccount> = Api::namespaced(client.clone(), ns);
    let account = match service_account.filter(|s| !s.is_empty()) {
        Some(sa) => Some(accounts.get_opt(sa).await?.ok_or_else(|| other(format!("ServiceAccount {sa} doesn't exist in {ns}.")))?),
        None => None,
    };

    let secret: Secret = serde_json::from_value(pull_secret(input)).map_err(|e| other(e.to_string()))?;
    secrets.create(&post_params(dry_run), &secret).await?;
    let mut done = vec![format!("Secret {ns}/{} created", input.name)];

    if let Some(mut sa) = account {
        let name = sa.metadata.name.clone().unwrap_or_default();
        let list = sa.image_pull_secrets.get_or_insert_with(Vec::new);
        if list.iter().any(|r| r.name == input.name) {
            done.push(format!("ServiceAccount {name} already lists it"));
        } else {
            list.push(LocalObjectReference { name: input.name.clone() });
            // A replace at the version just read: a concurrent edit fails
            // instead of being overwritten.
            accounts.replace(&name, &post_params(dry_run), &sa).await.map_err(|e| match &e {
                kube::Error::Api(s) if s.code == 409 => other(format!(
                    "The Secret was created, but ServiceAccount {name} changed while it was being updated. Add the pull secret to it again."
                )),
                _ => KubeError::from(e),
            })?;
            done.push(format!("added to ServiceAccount {name}'s imagePullSecrets"));
        }
    }
    if dry_run {
        done.push("dry run, nothing changed".into());
    }
    Ok(done)
}
