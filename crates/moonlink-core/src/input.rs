use std::future::Future;

use async_trait::async_trait;
use moonlink_protocol::InputEvent;

use crate::media::MediaSessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputContext {
    pub client_id: String,
    pub session_id: MediaSessionId,
}

#[async_trait]
pub trait InputSink: Send + Sync + 'static {
    async fn on_input(&self, context: InputContext, event: InputEvent);
}

#[async_trait]
impl<F, Fut> InputSink for F
where
    F: Send + Sync + 'static + Fn(InputContext, InputEvent) -> Fut,
    Fut: Future<Output = ()> + Send,
{
    async fn on_input(&self, context: InputContext, event: InputEvent) {
        self(context, event).await;
    }
}
