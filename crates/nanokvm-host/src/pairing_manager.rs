use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moonlink_core::{PairingPinProvider, PairingRequest};
use serde::Serialize;
use tokio::sync::{Mutex, oneshot};

const PAIRING_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct PendingPairing {
    pub client_id: String,
    pub display_name: Option<String>,
}

struct PendingRequest {
    pairing: PendingPairing,
    pin_sender: oneshot::Sender<String>,
}

#[derive(Clone, Default)]
pub struct PairingManager {
    pending: Arc<Mutex<HashMap<String, PendingRequest>>>,
}

impl PairingManager {
    pub async fn pending(&self) -> Vec<PendingPairing> {
        self.pending
            .lock()
            .await
            .values()
            .map(|request| request.pairing.clone())
            .collect()
    }

    pub async fn submit_pin(&self, client_id: &str, pin: &str) -> Result<(), &'static str> {
        if pin.len() != 4 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Moonlight PIN must contain four digits");
        }
        let request = self
            .pending
            .lock()
            .await
            .remove(client_id)
            .ok_or("pairing request is no longer pending")?;
        request
            .pin_sender
            .send(pin.to_owned())
            .map_err(|_| "pairing request is no longer pending")
    }

    pub async fn reject(&self, client_id: &str) -> bool {
        self.pending.lock().await.remove(client_id).is_some()
    }
}

#[async_trait]
impl PairingPinProvider for PairingManager {
    async fn request_pin(&self, request: PairingRequest) -> Option<String> {
        let client_id = request.client_id.clone();
        let (pin_sender, pin_receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            client_id.clone(),
            PendingRequest {
                pairing: PendingPairing {
                    client_id: request.client_id,
                    display_name: request.display_name,
                },
                pin_sender,
            },
        );
        println!("Moonlight pairing request pending for {client_id}");
        let pin = tokio::time::timeout(PAIRING_TIMEOUT, pin_receiver)
            .await
            .ok()
            .and_then(Result::ok);
        self.pending.lock().await.remove(&client_id);
        pin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_each_client_independently() {
        let manager = PairingManager::default();
        let first = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .request_pin(PairingRequest {
                        client_id: "first".to_owned(),
                        display_name: Some("Phone".to_owned()),
                    })
                    .await
            }
        });
        let second = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .request_pin(PairingRequest {
                        client_id: "second".to_owned(),
                        display_name: Some("Tablet".to_owned()),
                    })
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert_eq!(manager.pending().await.len(), 2);
        manager.submit_pin("second", "5678").await.unwrap();
        manager.submit_pin("first", "1234").await.unwrap();
        assert_eq!(first.await.unwrap().as_deref(), Some("1234"));
        assert_eq!(second.await.unwrap().as_deref(), Some("5678"));
    }
}
