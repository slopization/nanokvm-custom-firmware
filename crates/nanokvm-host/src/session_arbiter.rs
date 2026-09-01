use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOwner {
    Moonlight,
    Web,
}

#[derive(Debug, Default)]
struct State {
    owner: Option<(SessionOwner, u64)>,
    generation: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionArbiter(Arc<Mutex<State>>);

#[derive(Debug)]
pub struct SessionLease {
    state: Weak<Mutex<State>>,
    owner: SessionOwner,
    generation: u64,
}

impl SessionArbiter {
    pub fn owner(&self) -> Option<SessionOwner> {
        self.0
            .lock()
            .expect("session arbiter poisoned")
            .owner
            .map(|v| v.0)
    }

    pub fn acquire(&self, owner: SessionOwner) -> Result<SessionLease, SessionOwner> {
        let mut state = self.0.lock().expect("session arbiter poisoned");
        if let Some((current, _)) = state.owner {
            return Err(current);
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.owner = Some((owner, generation));
        Ok(SessionLease {
            state: Arc::downgrade(&self.0),
            owner,
            generation,
        })
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = state.lock().expect("session arbiter poisoned");
        if state.owner == Some((self.owner, self.generation)) {
            state.owner = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_exclusive_and_released_on_drop() {
        let arbiter = SessionArbiter::default();
        let lease = arbiter.acquire(SessionOwner::Web).unwrap();
        assert_eq!(arbiter.owner(), Some(SessionOwner::Web));
        assert_eq!(
            arbiter.acquire(SessionOwner::Moonlight).unwrap_err(),
            SessionOwner::Web
        );
        drop(lease);
        assert!(arbiter.acquire(SessionOwner::Moonlight).is_ok());
    }
}
