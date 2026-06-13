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

    pub async fn remove_target_instance(&self, target: &str, daemon_instance_id: &str) -> usize {
        let mut removed = Vec::new();
        {
            let mut guard = self.inner.write().await;
            let session_ids = guard
                .iter()
                .filter(|(_, record)| {
                    record.target == target && record.daemon_instance_id == daemon_instance_id
                })
                .map(|(session_id, _)| session_id.clone())
                .collect::<Vec<_>>();
            for session_id in session_ids {
                if let Some(record) = guard.remove(&session_id) {
                    removed.push(record);
                }
            }
        }

        let removed_count = removed.len();
        for record in removed {
            tracing::info!(
                session_id = %record.session_id,
                target = %record.target,
                daemon_session_id = %record.daemon_session_id,
                daemon_instance_id = %record.daemon_instance_id,
                "removed broker session mapping after target instance refresh invalidation"
            );
        }

        removed_count
    }
}

#[cfg(test)]
mod tests {
    use super::SessionStore;

    #[tokio::test]
    async fn remove_target_instance_preserves_other_target_and_new_instance_sessions() {
        let store = SessionStore::default();
        let old_target_session = store
            .insert(
                "target-a".to_string(),
                "daemon-session-old".to_string(),
                "daemon-old".to_string(),
                "cmd-old".to_string(),
            )
            .await;
        let new_target_session = store
            .insert(
                "target-a".to_string(),
                "daemon-session-new".to_string(),
                "daemon-new".to_string(),
                "cmd-new".to_string(),
            )
            .await;
        let other_target_session = store
            .insert(
                "target-b".to_string(),
                "daemon-session-other".to_string(),
                "daemon-old".to_string(),
                "cmd-other".to_string(),
            )
            .await;

        let removed = store.remove_target_instance("target-a", "daemon-old").await;

        assert_eq!(removed, 1);
        assert!(store.get(&old_target_session.session_id).await.is_none());
        assert!(store.get(&new_target_session.session_id).await.is_some());
        assert!(store.get(&other_target_session.session_id).await.is_some());
    }
}
