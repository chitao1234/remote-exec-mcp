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
        let session_id = remote_exec_host::ids::new_public_session_id();
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
        tracing::info!(
            session_id = %record.session_id,
            target = %record.target,
            daemon_session_id = %record.daemon_session_id,
            daemon_instance_id = %record.daemon_instance_id,
            "created broker session mapping"
        );
        record
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionRecord> {
        self.inner.read().await.get(session_id).cloned()
    }

    pub async fn remove(&self, session_id: &str) {
        if let Some(record) = self.inner.write().await.remove(session_id) {
            tracing::info!(
                session_id = %record.session_id,
                target = %record.target,
                daemon_session_id = %record.daemon_session_id,
                daemon_instance_id = %record.daemon_instance_id,
                "removed broker session mapping"
            );
        }
    }

    pub async fn remove_target(&self, target: &str) {
        let mut removed = Vec::new();
        {
            let mut guard = self.inner.write().await;
            let session_ids = guard
                .iter()
                .filter(|(_, record)| record.target == target)
                .map(|(session_id, _)| session_id.clone())
                .collect::<Vec<_>>();
            for session_id in session_ids {
                if let Some(record) = guard.remove(&session_id) {
                    removed.push(record);
                }
            }
        }

        for record in removed {
            tracing::info!(
                session_id = %record.session_id,
                target = %record.target,
                daemon_session_id = %record.daemon_session_id,
                daemon_instance_id = %record.daemon_instance_id,
                "removed broker session mapping after target refresh invalidation"
            );
        }
    }
}
