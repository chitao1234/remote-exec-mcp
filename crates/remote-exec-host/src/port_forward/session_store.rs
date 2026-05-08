use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::HostRpcError;

use super::error::rpc_error;
use super::session::SessionState;

#[derive(Clone, Default)]
pub struct TunnelSessionStore {
    pub(super) sessions: Arc<Mutex<HashMap<String, Arc<SessionState>>>>,
}

impl TunnelSessionStore {
    pub(super) async fn try_insert(
        &self,
        session: Arc<SessionState>,
        max_sessions: usize,
    ) -> Result<(), HostRpcError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.len() >= max_sessions {
            return Err(rpc_error(
                "port_tunnel_limit_exceeded",
                "retained port tunnel session limit reached",
            ));
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    pub(super) async fn get(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.get(session_id).cloned()
    }

    pub(super) async fn remove(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.remove(session_id)
    }
}
