use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRequest {
    pub client_id: String,
    pub display_name: Option<String>,
}

#[async_trait]
pub trait PairingPinProvider: Send + Sync + 'static {
    async fn request_pin(&self, request: PairingRequest) -> Option<String>;
}

#[derive(Clone)]
pub enum PairingPin {
    Static(String),
    InteractiveStdin,
    Provider(Arc<dyn PairingPinProvider>),
}

impl PairingPin {
    pub fn static_pin(pin: impl Into<String>) -> Self {
        Self::Static(pin.into())
    }

    pub fn provider(provider: impl PairingPinProvider) -> Self {
        Self::Provider(Arc::new(provider))
    }
}

impl fmt::Debug for PairingPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(_) => formatter.write_str("PairingPin::Static([redacted])"),
            Self::InteractiveStdin => formatter.write_str("PairingPin::InteractiveStdin"),
            Self::Provider(_) => formatter.write_str("PairingPin::Provider"),
        }
    }
}
