use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Default)]
pub(crate) struct MediaSessionManager {
    active: Option<ActiveMediaSession>,
}

struct ActiveMediaSession {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl MediaSessionManager {
    pub(crate) fn start(&mut self, shutdown: watch::Sender<bool>, task: JoinHandle<()>) {
        debug_assert!(self.active.is_none());
        self.active = Some(ActiveMediaSession { shutdown, task });
    }

    pub(crate) async fn stop(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        let _ = active.shutdown.send(true);
        if let Err(err) = active.task.await {
            eprintln!("media task failed to join: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn stop_waits_for_media_teardown() {
        let stopped = Arc::new(AtomicBool::new(false));
        let task_stopped = Arc::clone(&stopped);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            shutdown_rx.changed().await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            task_stopped.store(true, Ordering::Release);
        });
        let mut manager = MediaSessionManager::default();
        manager.start(shutdown, task);

        manager.stop().await;

        assert!(stopped.load(Ordering::Acquire));
    }
}
