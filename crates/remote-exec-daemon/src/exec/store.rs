use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::session::LiveSession;

type SharedSession = Arc<Mutex<LiveSession>>;

#[derive(Default, Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SharedSession>>>,
}

impl SessionStore {
    pub async fn insert(&self, session_id: String, session: LiveSession) {
        self.inner
            .write()
            .await
            .insert(session_id, Arc::new(Mutex::new(session)));
    }

    pub async fn lock(&self, session_id: &str) -> Option<OwnedMutexGuard<LiveSession>> {
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
    ) -> Option<OwnedMutexGuard<LiveSession>> {
        let guard = session.clone().lock_owned().await;
        let is_current = self
            .inner
            .read()
            .await
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &session));
        if is_current { Some(guard) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::SessionStore;
    use crate::exec::session;

    #[tokio::test]
    async fn lock_rejects_stale_snapshot_after_session_replacement() {
        let store = SessionStore::default();
        let session_id = "session-1";
        let cmd = vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "sleep 2".to_string(),
        ];

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
}
