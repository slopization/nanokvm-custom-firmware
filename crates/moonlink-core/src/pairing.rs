use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientIdentity {
    pub id: String,
    pub certificate_der: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedClient {
    pub identity: ClientIdentity,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    pub id: Uuid,
    pub certificate_pem: Vec<u8>,
    pub private_key_pem: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingDecision {
    Trusted,
    Rejected,
}

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("pairing store is unavailable")]
    StoreUnavailable,
    #[error("client is not trusted")]
    ClientNotTrusted,
}

#[async_trait]
pub trait PairingStore: Send + Sync + 'static {
    async fn server_identity(&self) -> Result<Option<ServerIdentity>, PairingError>;
    async fn put_server_identity(&self, identity: ServerIdentity) -> Result<(), PairingError>;
    async fn is_client_trusted(&self, certificate_der: &[u8]) -> Result<bool, PairingError>;
    async fn trusted_clients(&self) -> Result<Vec<TrustedClient>, PairingError>;
    async fn trust_client(&self, client: TrustedClient) -> Result<(), PairingError>;
    async fn remove_client(&self, client_id: &str) -> Result<(), PairingError>;
}

pub fn client_id_from_certificate(certificate_der: &[u8]) -> String {
    hex::encode(Sha256::digest(certificate_der))
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryPairingStore {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    server_identity: Option<ServerIdentity>,
    clients: HashMap<String, TrustedClient>,
}

#[async_trait]
impl PairingStore for InMemoryPairingStore {
    async fn server_identity(&self) -> Result<Option<ServerIdentity>, PairingError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| PairingError::StoreUnavailable)?
            .server_identity
            .clone())
    }

    async fn put_server_identity(&self, identity: ServerIdentity) -> Result<(), PairingError> {
        self.inner
            .write()
            .map_err(|_| PairingError::StoreUnavailable)?
            .server_identity = Some(identity);
        Ok(())
    }

    async fn is_client_trusted(&self, certificate_der: &[u8]) -> Result<bool, PairingError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| PairingError::StoreUnavailable)?
            .clients
            .values()
            .any(|client| client.identity.certificate_der == certificate_der))
    }

    async fn trust_client(&self, client: TrustedClient) -> Result<(), PairingError> {
        self.inner
            .write()
            .map_err(|_| PairingError::StoreUnavailable)?
            .clients
            .insert(client.identity.id.clone(), client);
        Ok(())
    }

    async fn trusted_clients(&self) -> Result<Vec<TrustedClient>, PairingError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| PairingError::StoreUnavailable)?
            .clients
            .values()
            .cloned()
            .collect())
    }

    async fn remove_client(&self, client_id: &str) -> Result<(), PairingError> {
        self.inner
            .write()
            .map_err(|_| PairingError::StoreUnavailable)?
            .clients
            .remove(client_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_trusted_clients() {
        let store = InMemoryPairingStore::default();
        let client = TrustedClient {
            identity: ClientIdentity {
                id: "client-1".to_owned(),
                certificate_der: vec![1, 2, 3],
            },
            display_name: Some("client".to_owned()),
        };
        store.trust_client(client.clone()).await.unwrap();
        let second = TrustedClient {
            identity: ClientIdentity {
                id: "client-2".to_owned(),
                certificate_der: vec![4, 5, 6],
            },
            display_name: Some("second".to_owned()),
        };
        store.trust_client(second).await.unwrap();
        assert!(store.is_client_trusted(&[1, 2, 3]).await.unwrap());
        assert_eq!(store.trusted_clients().await.unwrap().len(), 2);
    }
}
