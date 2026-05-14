use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use remote_exec_proto::rpc::RpcErrorCode;

use crate::HostRpcError;

use super::error::request_error;
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
            return Err(request_error(
                RpcErrorCode::PortTunnelLimitExceeded,
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

    #[cfg(test)]
    pub(super) async fn contains(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains_key(session_id)
    }
}
