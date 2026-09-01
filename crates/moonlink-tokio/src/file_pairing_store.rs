use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use moonlink_core::{
    ClientIdentity, PairingError, PairingStore, ServerIdentity, TrustedClient,
    client_id_from_certificate,
};
use openssl::x509::X509;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct FilePairingStore {
    path: Arc<PathBuf>,
    state: Arc<Mutex<StoredPairingState>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoredPairingState {
    server_identity: Option<ServerIdentityRecord>,
    clients: HashMap<String, TrustedClientRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerIdentityRecord {
    id: uuid::Uuid,
    certificate_pem: Vec<u8>,
    private_key_pem: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustedClientRecord {
    id: String,
    certificate_der: Vec<u8>,
    display_name: Option<String>,
}

impl FilePairingStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, PairingError> {
        let path = path.into();
        let mut state = read_state(&path).await?;
        let changed = normalize_client_ids(&mut state);
        let store = Self {
            path: Arc::new(path),
            state: Arc::new(Mutex::new(state)),
        };
        if changed {
            let state = store.state.lock().await;
            store.save_state(&state).await?;
        }
        Ok(store)
    }

    async fn save_state(&self, state: &StoredPairingState) -> Result<(), PairingError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(store_error)?;
        let bytes = serde_json::to_vec_pretty(state).map_err(store_error)?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true).mode(0o600);
        let mut file = options.open(&temporary).await.map_err(store_error)?;
        file.write_all(&bytes).await.map_err(store_error)?;
        file.sync_all().await.map_err(store_error)?;
        drop(file);
        tokio::fs::rename(&temporary, self.path.as_ref())
            .await
            .map_err(store_error)?;
        tokio::fs::File::open(parent)
            .await
            .map_err(store_error)?
            .sync_all()
            .await
            .map_err(store_error)
    }
}

#[async_trait]
impl PairingStore for FilePairingStore {
    async fn server_identity(&self) -> Result<Option<ServerIdentity>, PairingError> {
        Ok(self
            .state
            .lock()
            .await
            .server_identity
            .clone()
            .map(Into::into))
    }

    async fn put_server_identity(&self, identity: ServerIdentity) -> Result<(), PairingError> {
        let mut state = self.state.lock().await;
        state.server_identity = Some(identity.into());
        self.save_state(&state).await
    }

    async fn is_client_trusted(&self, certificate_der: &[u8]) -> Result<bool, PairingError> {
        Ok(self
            .state
            .lock()
            .await
            .clients
            .values()
            .any(|client| client.certificate_der == certificate_der))
    }

    async fn trusted_clients(&self) -> Result<Vec<TrustedClient>, PairingError> {
        Ok(self
            .state
            .lock()
            .await
            .clients
            .values()
            .cloned()
            .map(Into::into)
            .collect())
    }

    async fn trust_client(&self, client: TrustedClient) -> Result<(), PairingError> {
        let mut state = self.state.lock().await;
        state
            .clients
            .insert(client.identity.id.clone(), client.into());
        self.save_state(&state).await
    }

    async fn remove_client(&self, client_id: &str) -> Result<(), PairingError> {
        let mut state = self.state.lock().await;
        state.clients.remove(client_id);
        self.save_state(&state).await
    }
}

async fn read_state(path: &Path) -> Result<StoredPairingState, PairingError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(store_error),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(StoredPairingState::default()),
        Err(err) => Err(store_error(err)),
    }
}

fn normalize_client_ids(state: &mut StoredPairingState) -> bool {
    let previous = std::mem::take(&mut state.clients);
    let mut changed = false;
    for (key, mut client) in previous {
        if let Ok(certificate) = X509::from_pem(&client.certificate_der)
            && let Ok(certificate_der) = certificate.to_der()
        {
            client.certificate_der = certificate_der;
        }
        let id = client_id_from_certificate(&client.certificate_der);
        changed |= key != id || client.id != id;
        client.id.clone_from(&id);
        state.clients.insert(id, client);
    }
    changed
}

fn store_error(_error: impl std::fmt::Display) -> PairingError {
    PairingError::StoreUnavailable
}

impl From<ServerIdentityRecord> for ServerIdentity {
    fn from(record: ServerIdentityRecord) -> Self {
        Self {
            id: record.id,
            certificate_pem: record.certificate_pem,
            private_key_pem: record.private_key_pem,
        }
    }
}

impl From<ServerIdentity> for ServerIdentityRecord {
    fn from(identity: ServerIdentity) -> Self {
        Self {
            id: identity.id,
            certificate_pem: identity.certificate_pem,
            private_key_pem: identity.private_key_pem,
        }
    }
}

impl From<TrustedClientRecord> for TrustedClient {
    fn from(record: TrustedClientRecord) -> Self {
        Self {
            identity: ClientIdentity {
                id: record.id,
                certificate_der: record.certificate_der,
            },
            display_name: record.display_name,
        }
    }
}

impl From<TrustedClient> for TrustedClientRecord {
    fn from(client: TrustedClient) -> Self {
        Self {
            id: client.identity.id,
            certificate_der: client.identity.certificate_der,
            display_name: client.display_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_shared_moonlight_unique_id_to_certificate_id() {
        let certificate_der = vec![1, 2, 3, 4];
        let mut state = StoredPairingState::default();
        state.clients.insert(
            "0123456789ABCDEF".to_owned(),
            TrustedClientRecord {
                id: "0123456789ABCDEF".to_owned(),
                certificate_der: certificate_der.clone(),
                display_name: Some("Phone".to_owned()),
            },
        );
        assert!(normalize_client_ids(&mut state));
        let id = client_id_from_certificate(&certificate_der);
        assert_eq!(state.clients.get(&id).unwrap().id, id);
    }
}
