use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::session::LiveSession;

type SharedSession = Arc<Mutex<LiveSession>>;
const DEFAULT_SESSION_LIMIT: usize = 64;

#[derive(Clone)]
struct SessionEntry {
    session: SharedSession,
    last_touched_at: Instant,
}

#[derive(Clone)]
struct Candidate {
    session_id: String,
    session: SharedSession,
    last_touched_at: Instant,
}

#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SessionEntry>>>,
    limit: usize,
}

pub struct SessionLease {
    inner: Arc<RwLock<HashMap<String, SessionEntry>>>,
    session_id: String,
    session: SharedSession,
    guard: OwnedMutexGuard<LiveSession>,
}

impl SessionStore {
    pub fn new(limit: usize) -> Self {
        assert!(limit > 0, "session store limit must be greater than zero");
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            limit,
        }
    }

    pub async fn insert(&self, session_id: String, session: LiveSession) {
        self.prune_for_insert().await;
        self.inner
            .write()
            .await
            .insert(
                session_id,
                SessionEntry {
                    session: Arc::new(Mutex::new(session)),
                    last_touched_at: Instant::now(),
                },
            );
    }

    pub async fn lock(&self, session_id: &str) -> Option<SessionLease> {
        let session = self.inner.read().await.get(session_id)?.session.clone();
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
            .is_some_and(|current| Arc::ptr_eq(&current.session, &session));
        if is_current {
            self.touch_if_current(session_id, &session).await;
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

    async fn touch_if_current(&self, session_id: &str, session: &SharedSession) {
        let mut sessions = self.inner.write().await;
        if let Some(entry) = sessions.get_mut(session_id)
            && Arc::ptr_eq(&entry.session, session)
        {
            entry.last_touched_at = Instant::now();
        }
    }

    async fn prune_for_insert(&self) {
        loop {
            let snapshot = {
                let sessions = self.inner.read().await;
                if sessions.len() < self.limit {
                    return;
                }
                sessions
                    .iter()
                    .map(|(session_id, entry)| Candidate {
                        session_id: session_id.clone(),
                        session: entry.session.clone(),
                        last_touched_at: entry.last_touched_at,
                    })
                    .collect::<Vec<_>>()
            };

            let victim = self
                .find_oldest_exited(&snapshot)
                .await
                .or_else(|| {
                    snapshot
                        .iter()
                        .min_by_key(|candidate| candidate.last_touched_at)
                        .cloned()
                });

            let Some(victim) = victim else {
                return;
            };

            let removed = {
                let mut sessions = self.inner.write().await;
                let is_current = sessions
                    .get(&victim.session_id)
                    .is_some_and(|current| Arc::ptr_eq(&current.session, &victim.session));
                if is_current {
                    sessions.remove(&victim.session_id)
                } else {
                    None
                }
            };

            if let Some(removed) = removed {
                let mut guard = removed.session.lock_owned().await;
                let _ = guard.terminate().await;
                return;
            }
        }
    }

    async fn find_oldest_exited(&self, snapshot: &[Candidate]) -> Option<Candidate> {
        let mut exited = Vec::new();

        for candidate in snapshot {
            let mut guard = candidate.session.clone().lock_owned().await;
            if guard.has_exited().await.unwrap_or(false) {
                exited.push(candidate.clone());
            }
        }

        exited
            .into_iter()
            .min_by_key(|candidate| candidate.last_touched_at)
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new(DEFAULT_SESSION_LIMIT)
    }
}

impl SessionLease {
    pub async fn retire(self) -> bool {
        let mut sessions = self.inner.write().await;
        let is_current = sessions
            .get(&self.session_id)
            .is_some_and(|current| Arc::ptr_eq(&current.session, &self.session));
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
    use std::time::Duration;

    use super::SessionStore;
    use crate::exec::session;

    const TEST_SHELL: &str = "/bin/sh";

    fn spawn_pipe_session(script: &str) -> session::LiveSession {
        session::spawn(
            &[TEST_SHELL.to_string(), "-c".to_string(), script.to_string()],
            Path::new("/"),
            false,
        )
        .expect("session should spawn")
    }

    #[tokio::test]
    async fn lock_rejects_stale_snapshot_after_session_replacement() {
        let store = SessionStore::default();
        let session_id = "session-1";
        let cmd = vec![
            TEST_SHELL.to_string(),
            "-c".to_string(),
            "sleep 2".to_string(),
        ];

        let first = session::spawn(&cmd, Path::new("/"), false).expect("first session");
        store.insert(session_id.to_string(), first).await;
        let stale = {
            let sessions = store.inner.read().await;
            sessions
                .get(session_id)
                .map(|entry| entry.session.clone())
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
        let cmd = vec![
            TEST_SHELL.to_string(),
            "-c".to_string(),
            "sleep 2".to_string(),
        ];

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

    #[tokio::test]
    async fn lock_refreshes_recency_so_older_live_session_is_pruned() {
        let store = SessionStore::new(2);
        store
            .insert("session-1".to_string(), spawn_pipe_session("sleep 2"))
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        store
            .insert("session-2".to_string(), spawn_pipe_session("sleep 2"))
            .await;

        let lease = store.lock("session-1").await.expect("session-1 should exist");
        drop(lease);
        tokio::time::sleep(Duration::from_millis(10)).await;

        store
            .insert("session-3".to_string(), spawn_pipe_session("sleep 2"))
            .await;

        assert!(store.lock("session-1").await.is_some());
        assert!(store.lock("session-2").await.is_none());
        assert!(store.lock("session-3").await.is_some());
    }

    #[tokio::test]
    async fn insert_prunes_exited_session_before_live_session() {
        let store = SessionStore::new(2);
        store
            .insert("session-1".to_string(), spawn_pipe_session("printf done"))
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        store
            .insert("session-2".to_string(), spawn_pipe_session("sleep 2"))
            .await;

        store
            .insert("session-3".to_string(), spawn_pipe_session("sleep 2"))
            .await;

        assert!(store.lock("session-1").await.is_none());
        assert!(store.lock("session-2").await.is_some());
        assert!(store.lock("session-3").await.is_some());
    }
}
