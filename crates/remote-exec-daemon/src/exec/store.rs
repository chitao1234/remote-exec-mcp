use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::session::LiveSession;

type SharedSession = Arc<Mutex<LiveSession>>;
const DEFAULT_SESSION_LIMIT: usize = 64;
const RECENT_PROTECTION_COUNT: usize = 8;
const WARNING_THRESHOLD: usize = 60;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsertOutcome {
    pub crossed_warning_threshold: bool,
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
        let crossed_warning_threshold = self.crosses_warning_threshold().await;
        self.prune_for_insert().await;
        self.inner.write().await.insert(
            session_id,
            SessionEntry {
                session: Arc::new(Mutex::new(session)),
                last_touched_at: Instant::now(),
            },
        );
        InsertOutcome {
            crossed_warning_threshold,
        }
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

    async fn crosses_warning_threshold(&self) -> bool {
        let current_len = self.inner.read().await.len();
        current_len < WARNING_THRESHOLD && current_len + 1 >= WARNING_THRESHOLD
    }

    fn protected_recent_count(&self) -> usize {
        self.limit.saturating_sub(1).min(RECENT_PROTECTION_COUNT)
    }

    async fn prune_for_insert(&self) {
        loop {
            let snapshot = {
                let sessions = self.inner.read().await;
                if sessions.len() < self.limit {
                    return;
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
                snapshot
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
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::time::Duration;

    use super::SessionStore;
    use crate::exec::session;

    #[cfg(unix)]
    const TEST_SHELL: &str = "/bin/sh";
    #[cfg(windows)]
    const TEST_SHELL: &str = "cmd.exe";

    fn test_workdir() -> &'static Path {
        static WORKDIR: OnceLock<PathBuf> = OnceLock::new();
        WORKDIR
            .get_or_init(|| std::env::current_dir().expect("current directory should resolve"))
            .as_path()
    }

    fn shell_argv(script: &str) -> Vec<String> {
        #[cfg(unix)]
        {
            vec![TEST_SHELL.to_string(), "-c".to_string(), script.to_string()]
        }

        #[cfg(windows)]
        {
            vec![
                TEST_SHELL.to_string(),
                "/D".to_string(),
                "/C".to_string(),
                script.to_string(),
            ]
        }
    }

    fn sleep_script(seconds: u64) -> String {
        #[cfg(unix)]
        {
            format!("sleep {seconds}")
        }

        #[cfg(windows)]
        {
            format!("ping -n {} 127.0.0.1 >nul", seconds + 1)
        }
    }

    fn completed_script() -> &'static str {
        #[cfg(unix)]
        {
            "printf done"
        }

        #[cfg(windows)]
        {
            "echo done"
        }
    }

    fn spawn_pipe_session(script: &str) -> session::LiveSession {
        session::spawn(
            &shell_argv(script),
            test_workdir(),
            false,
            &crate::config::ProcessEnvironment::capture_current(),
        )
        .expect("session should spawn")
    }

    #[tokio::test]
    async fn lock_rejects_stale_snapshot_after_session_replacement() {
        let store = SessionStore::default();
        let session_id = "session-1";
        let cmd = shell_argv(&sleep_script(2));

        let first = session::spawn(
            &cmd,
            test_workdir(),
            false,
            &crate::config::ProcessEnvironment::capture_current(),
        )
        .expect("first session");
        store.insert(session_id.to_string(), first).await;
        let stale = {
            let sessions = store.inner.read().await;
            sessions
                .get(session_id)
                .map(|entry| entry.session.clone())
                .expect("session must exist for stale snapshot")
        };

        let replacement = session::spawn(
            &cmd,
            test_workdir(),
            false,
            &crate::config::ProcessEnvironment::capture_current(),
        )
        .expect("replacement session");
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
        let cmd = shell_argv(&sleep_script(2));

        let session = session::spawn(
            &cmd,
            test_workdir(),
            false,
            &crate::config::ProcessEnvironment::capture_current(),
        )
        .expect("session");
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
            .insert(
                "session-1".to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        store
            .insert(
                "session-2".to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;

        let lease = store
            .lock("session-1")
            .await
            .expect("session-1 should exist");
        drop(lease);
        tokio::time::sleep(Duration::from_millis(10)).await;

        store
            .insert(
                "session-3".to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;

        assert!(store.lock("session-1").await.is_some());
        assert!(store.lock("session-2").await.is_none());
        assert!(store.lock("session-3").await.is_some());
    }

    #[tokio::test]
    async fn insert_prunes_exited_session_before_live_session() {
        let store = SessionStore::new(2);
        store
            .insert(
                "session-1".to_string(),
                spawn_pipe_session(completed_script()),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        store
            .insert(
                "session-2".to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;

        store
            .insert(
                "session-3".to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;

        assert!(store.lock("session-1").await.is_none());
        assert!(store.lock("session-2").await.is_some());
        assert!(store.lock("session-3").await.is_some());
    }

    #[tokio::test]
    async fn insert_protects_eight_most_recent_sessions() {
        let store = SessionStore::new(10);
        for index in 0..10 {
            store
                .insert(
                    format!("session-{index}"),
                    spawn_pipe_session(&sleep_script(30)),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        store.lock("session-9").await.expect("session-9");
        store.lock("session-8").await.expect("session-8");
        store.lock("session-7").await.expect("session-7");
        store.lock("session-6").await.expect("session-6");
        store.lock("session-5").await.expect("session-5");
        store.lock("session-4").await.expect("session-4");
        store.lock("session-3").await.expect("session-3");
        store.lock("session-2").await.expect("session-2");

        let outcome = store
            .insert(
                "session-10".to_string(),
                spawn_pipe_session(&sleep_script(30)),
            )
            .await;

        assert!(!outcome.crossed_warning_threshold);
        assert!(store.lock("session-0").await.is_none());
        for protected in [
            "session-2",
            "session-3",
            "session-4",
            "session-5",
            "session-6",
            "session-7",
            "session-8",
            "session-9",
        ] {
            assert!(
                store.lock(protected).await.is_some(),
                "{protected} should remain protected"
            );
        }
    }

    #[tokio::test]
    async fn insert_prunes_oldest_exited_non_protected_session_before_live_one() {
        let store = SessionStore::new(10);
        store
            .insert(
                "session-0".to_string(),
                spawn_pipe_session(completed_script()),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        for index in 1..10 {
            store
                .insert(
                    format!("session-{index}"),
                    spawn_pipe_session(&sleep_script(30)),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let outcome = store
            .insert(
                "session-10".to_string(),
                spawn_pipe_session(&sleep_script(30)),
            )
            .await;

        assert!(!outcome.crossed_warning_threshold);
        assert!(store.lock("session-0").await.is_none());
        assert!(store.lock("session-1").await.is_some());
    }

    #[tokio::test]
    async fn insert_reports_warning_only_when_crossing_threshold() {
        let store = SessionStore::new(64);

        for index in 0..59 {
            let outcome = store
                .insert(
                    format!("session-{index}"),
                    spawn_pipe_session(&sleep_script(30)),
                )
                .await;
            assert!(
                !outcome.crossed_warning_threshold,
                "unexpected warning at {index}"
            );
        }

        let crossing = store
            .insert(
                "session-59".to_string(),
                spawn_pipe_session(&sleep_script(30)),
            )
            .await;
        assert!(crossing.crossed_warning_threshold);

        let above_threshold = store
            .insert(
                "session-60".to_string(),
                spawn_pipe_session(&sleep_script(30)),
            )
            .await;
        assert!(!above_threshold.crossed_warning_threshold);

        store.remove("session-0").await;
        store.remove("session-1").await;

        let recrossing = store
            .insert(
                "session-61".to_string(),
                spawn_pipe_session(&sleep_script(30)),
            )
            .await;
        assert!(recrossing.crossed_warning_threshold);
    }
}
