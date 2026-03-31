use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use super::session::LiveSession;

pub type SharedSession = Arc<Mutex<LiveSession>>;

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

    pub async fn get(&self, session_id: &str) -> Option<SharedSession> {
        self.inner.read().await.get(session_id).cloned()
    }

    pub async fn remove(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}
