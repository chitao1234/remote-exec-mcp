use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::session::LiveSession;

type SharedSession = Arc<Mutex<LiveSession>>;
const DEFAULT_SESSION_LIMIT: usize = 64;
const RECENT_PROTECTION_COUNT: usize = 8;
const WARNING_THRESHOLD_HEADROOM: usize = 4;

fn session_matches(entry: &SessionEntry, session: &SharedSession) -> bool {
    Arc::ptr_eq(&entry.session, session)
}

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

pub struct InsertOutcome {
    pub crossed_warning_threshold: bool,
    pub warning_threshold: usize,
    pub lease: SessionLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLockError {
    UnknownSession,
    TimedOut,
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

    pub async fn insert(&self, session_id: String, session: LiveSession) -> InsertOutcome {
        let warning_threshold = self.warning_threshold();
        let crossed_warning_threshold = self.crosses_warning_threshold(warning_threshold).await;
        self.prune_for_insert().await;
        let session_id_for_log = session_id.clone();
        let shared_session = Arc::new(Mutex::new(session));
        let guard = shared_session.clone().lock_owned().await;
        let mut sessions = self.inner.write().await;
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                session: shared_session.clone(),
                last_touched_at: Instant::now(),
            },
        );
        tracing::info!(
            session_id = %session_id_for_log,
            open_sessions = sessions.len(),
            crossed_warning_threshold,
            "stored exec session"
        );
        InsertOutcome {
            crossed_warning_threshold,
            warning_threshold,
            lease: SessionLease {
                inner: self.inner.clone(),
                session_id,
                session: shared_session,
                guard,
            },
        }
    }

    pub async fn lock(&self, session_id: &str) -> Option<SessionLease> {
        let session = self.inner.read().await.get(session_id)?.session.clone();
        self.lock_if_current(session_id, session).await
    }

    pub async fn lock_with_timeout(
        &self,
        session_id: &str,
        timeout: std::time::Duration,
    ) -> Result<SessionLease, SessionLockError> {
        let session = self
            .inner
            .read()
            .await
            .get(session_id)
            .map(|entry| entry.session.clone())
            .ok_or(SessionLockError::UnknownSession)?;
        self.lock_if_current_with_timeout(session_id, session, timeout)
            .await
    }

    pub async fn remove(&self, session_id: &str) {
        let mut sessions = self.inner.write().await;
        if sessions.remove(session_id).is_some() {
            tracing::info!(
                session_id,
                open_sessions = sessions.len(),
                "removed exec session"
            );
        }
    }

    async fn lock_if_current(
        &self,
        session_id: &str,
        session: SharedSession,
    ) -> Option<SessionLease> {
        self.lock_if_current_after_guard(session_id, session.clone(), session.lock_owned().await)
            .await
    }

    async fn lock_if_current_with_timeout(
        &self,
        session_id: &str,
        session: SharedSession,
        timeout: std::time::Duration,
    ) -> Result<SessionLease, SessionLockError> {
        let guard = tokio::time::timeout(timeout, session.clone().lock_owned())
            .await
            .map_err(|_| SessionLockError::TimedOut)?;
        self.lock_if_current_after_guard(session_id, session, guard)
            .await
            .ok_or(SessionLockError::UnknownSession)
    }

    async fn lock_if_current_after_guard(
        &self,
        session_id: &str,
        session: SharedSession,
        guard: OwnedMutexGuard<LiveSession>,
    ) -> Option<SessionLease> {
        // Recheck the store entry after acquiring the session lock: the session may have been
        // removed or replaced while we were waiting on the mutex, so the lease is valid only if
        // the store still points at this exact session instance.
        let is_current = self
            .inner
            .read()
            .await
            .get(session_id)
            .is_some_and(|current| session_matches(current, &session));
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
        if let Some(entry) = sessions.get_mut(session_id) {
            if session_matches(entry, session) {
                entry.last_touched_at = Instant::now();
            }
        }
    }

    fn warning_threshold(&self) -> usize {
        self.limit.saturating_sub(WARNING_THRESHOLD_HEADROOM)
    }

    async fn crosses_warning_threshold(&self, warning_threshold: usize) -> bool {
        let current_len = self.inner.read().await.len();
        current_len < warning_threshold && current_len + 1 >= warning_threshold
    }

    fn protected_recent_count(&self) -> usize {
        self.limit.saturating_sub(1).min(RECENT_PROTECTION_COUNT)
    }

    async fn prune_for_insert(&self) {
        loop {
            let Some(snapshot) = self.snapshot_for_prune().await else {
                return;
            };

            let protected = self.protected_recent_count();
            let prunable = &snapshot[..snapshot.len().saturating_sub(protected)];
            let victim = self
                .find_oldest_exited(prunable)
                .await
                .or_else(|| prunable.first().cloned());

            let Some(victim) = victim else {
                return;
            };

            let removed = self
                .remove_if_current(&victim.session_id, &victim.session)
                .await;

            if let Some(removed) = removed {
                let mut guard = removed.session.lock_owned().await;
                let _ = guard.terminate().await;
                drop(guard);
                let open_sessions_after_prune = self.inner.read().await.len();
                tracing::warn!(
                    victim_session_id = %victim.session_id,
                    open_sessions_after_prune,
                    "pruned exec session to respect limit"
                );
                return;
            }
        }
    }

    async fn snapshot_for_prune(&self) -> Option<Vec<Candidate>> {
        let sessions = self.inner.read().await;
        if sessions.len() < self.limit {
            return None;
        }

        let mut snapshot = sessions
            .iter()
            .map(|(session_id, entry)| Candidate {
                session_id: session_id.clone(),
                session: entry.session.clone(),
                last_touched_at: entry.last_touched_at,
            })
            .collect::<Vec<_>>();
        snapshot.sort_by_key(|candidate| candidate.last_touched_at);
        Some(snapshot)
    }

    async fn remove_if_current(
        &self,
        session_id: &str,
        session: &SharedSession,
    ) -> Option<SessionEntry> {
        let mut sessions = self.inner.write().await;
        let is_current = sessions
            .get(session_id)
            .is_some_and(|current| session_matches(current, session));
        if is_current {
            sessions.remove(session_id)
        } else {
            None
        }
    }

    async fn find_oldest_exited(&self, snapshot: &[Candidate]) -> Option<Candidate> {
        for candidate in snapshot {
            let mut guard = candidate.session.clone().lock_owned().await;
            if guard.has_exited().await.unwrap_or(false) {
                return Some(candidate.clone());
            }
        }

        None
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
            .is_some_and(|current| session_matches(current, &self.session));
        if is_current {
            sessions.remove(&self.session_id);
            tracing::info!(
                session_id = %self.session_id,
                open_sessions = sessions.len(),
                "retired exec session"
            );
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
mod tests;
