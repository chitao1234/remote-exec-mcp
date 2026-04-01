use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::session::LiveSession;

type SharedSession = Arc<Mutex<LiveSession>>;

#[derive(Default, Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SharedSession>>>,
}

pub struct SessionLease {
    inner: Arc<RwLock<HashMap<String, SharedSession>>>,
    session_id: String,
    session: SharedSession,
    guard: OwnedMutexGuard<LiveSession>,
}

impl SessionStore {
    pub async fn insert(&self, session_id: String, session: LiveSession) {
        self.inner
            .write()
            .await
            .insert(session_id, Arc::new(Mutex::new(session)));
    }

    pub async fn lock(&self, session_id: &str) -> Option<SessionLease> {
        let session = self.inner.read().await.get(session_id).cloned()?;
        self.lock_if_current(session_id, session).await
    }

    pub async fn remove(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }

    async fn lock_if_current(
        &self,
        session_id: &str,
        session: SharedSession,
    ) -> Option<SessionLease> {
        let guard = session.clone().lock_owned().await;
        let is_current = self
            .inner
            .read()
            .await
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &session));
        if is_current {
            Some(SessionLease {
                inner: self.inner.clone(),
                session_id: session_id.to_string(),
                session,
                guard,
            })
        } else {
            None
        }
    }
}

impl SessionLease {
    pub async fn retire(self) -> bool {
        let mut sessions = self.inner.write().await;
        let is_current = sessions
            .get(&self.session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.session));
        if is_current {
            sessions.remove(&self.session_id);
            true
        } else {
            false
        }
    }
}

impl Deref for SessionLease {
    type Target = LiveSession;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for SessionLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::SessionStore;
    use crate::exec::session;

    const TEST_SHELL: &str = "/bin/sh";

    #[tokio::test]
    async fn lock_rejects_stale_snapshot_after_session_replacement() {
        let store = SessionStore::default();
        let session_id = "session-1";
        let cmd = vec![TEST_SHELL.to_string(), "-c".to_string(), "sleep 2".to_string()];

        let first = session::spawn(&cmd, Path::new("/"), false).expect("first session");
        store.insert(session_id.to_string(), first).await;
        let stale = {
            let sessions = store.inner.read().await;
            sessions
                .get(session_id)
                .cloned()
                .expect("session must exist for stale snapshot")
        };

        let replacement = session::spawn(&cmd, Path::new("/"), false).expect("replacement session");
        store.insert(session_id.to_string(), replacement).await;

        let stale_guard = store.lock_if_current(session_id, stale).await;
        assert!(
            stale_guard.is_none(),
            "stale session snapshot must not be lockable after replacement"
        );

        let current_guard = store.lock(session_id).await;
        assert!(
            current_guard.is_some(),
            "current session should still be lockable after replacement"
        );
    }

    #[tokio::test]
    async fn retire_prevents_waiting_lock_from_reusing_session() {
        let store = SessionStore::default();
        let session_id = "session-1";
        let cmd = vec![TEST_SHELL.to_string(), "-c".to_string(), "sleep 2".to_string()];

        let session = session::spawn(&cmd, Path::new("/"), false).expect("session");
        store.insert(session_id.to_string(), session).await;
        let lease = store.lock(session_id).await.expect("lease");

        let waiter_store = store.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            let _ = started_tx.send(());
            waiter_store.lock(session_id).await
        });

        started_rx.await.expect("waiter should start");
        tokio::task::yield_now().await;
        lease.retire().await;

        let waiter_result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should complete")
            .expect("waiter join");
        assert!(
            waiter_result.is_none(),
            "waiting lock must observe retired session as unavailable"
        );
    }
}
