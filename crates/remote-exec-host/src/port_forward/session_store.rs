use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::session::SessionState;

#[derive(Clone, Default)]
pub struct TunnelSessionStore {
    pub(super) sessions: Arc<Mutex<HashMap<String, Arc<SessionState>>>>,
}

impl TunnelSessionStore {
    pub(super) async fn insert(&self, session: Arc<SessionState>) {
        self.sessions
            .lock()
            .await
            .insert(session.id.clone(), session);
    }

    pub(super) async fn get(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.get(session_id).cloned()
    }

    pub(super) async fn remove(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.remove(session_id)
    }
}
