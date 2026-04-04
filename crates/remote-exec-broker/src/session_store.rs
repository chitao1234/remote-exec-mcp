use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub target: String,
    pub daemon_session_id: String,
    pub daemon_instance_id: String,
    pub session_command: String,
}

#[derive(Default, Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SessionRecord>>>,
}

impl SessionStore {
    pub async fn insert(
        &self,
        target: String,
        daemon_session_id: String,
        daemon_instance_id: String,
        session_command: String,
    ) -> SessionRecord {
        let session_id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        let record = SessionRecord {
            session_id: session_id.clone(),
            target,
            daemon_session_id,
            daemon_instance_id,
            session_command,
        };
        self.inner
            .write()
            .await
            .insert(session_id.clone(), record.clone());
        record
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionRecord> {
        self.inner.read().await.get(session_id).cloned()
    }

    pub async fn remove(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}
