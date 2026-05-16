use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortPhase, ForwardPortSideHealth, ForwardPortSideRole,
    ForwardPortSideState, ForwardPortStatus, Timestamp,
};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::supervisor::{ListenSessionControl, close_listen_session, wait_for_forward_task_stop};

const RECONNECT_LIMIT_EXCEEDED: &str =
    "port_forward_limit_exceeded: broker reconnecting forward limit reached";

#[derive(Clone, Default)]
pub struct PortForwardStore {
    state: Arc<RwLock<PortForwardStoreState>>,
    close_lock: Arc<Mutex<()>>,
}

impl PortForwardStore {
    pub async fn insert(&self, record: PortForwardRecord) {
        let mut state = self.state.write().await;
        state.insert_record(record);
    }

    pub async fn list(&self, filter: &PortForwardFilter) -> Vec<ForwardPortEntry> {
        let mut entries = self
            .state
            .read()
            .await
            .entries
            .values()
            .filter(|record| filter.matches(&record.entry))
            .map(|record| record.entry.clone())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.forward_id.cmp(&right.forward_id));
        entries
    }

    pub async fn open_count(&self) -> usize {
        self.state.read().await.entries.len()
    }

    pub async fn side_pair_count(&self, listen_side: &str, connect_side: &str) -> usize {
        self.state
            .read()
            .await
            .entries
            .values()
            .filter(|record| {
                record.entry.listen_side == listen_side && record.entry.connect_side == connect_side
            })
            .count()
    }

    pub async fn close(&self, forward_ids: &[ForwardId]) -> anyhow::Result<Vec<ForwardPortEntry>> {
        let _close_guard = self.close_lock.lock().await;
        let candidates = self.take_close_candidates(forward_ids).await?;
        drop(_close_guard);

        let mut candidates = candidates;
        let mut closed = Vec::with_capacity(candidates.len());
        while !candidates.is_empty() {
            let candidate = candidates.remove(0);
            let PortForwardCloseCandidate {
                forward_id,
                mut record,
            } = candidate;
            if let Err(err) = close_handle(&record.close_handle).await {
                let error = format!("closing port forward `{}`: {err:#}", forward_id);
                mark_entry_failed(&mut record.entry, error.clone());
                let mut restore_candidates = vec![PortForwardCloseCandidate { forward_id, record }];
                restore_candidates.extend(candidates);
                self.restore_close_candidates(restore_candidates).await;
                return Err(anyhow::anyhow!(error));
            }
            closed.push(closed_entry(record.entry));
        }
        Ok(closed)
    }

    async fn validated_unique_close_ids(
        &self,
        forward_ids: &[ForwardId],
    ) -> anyhow::Result<Vec<ForwardId>> {
        let state = self.state.read().await;
        let mut seen = HashSet::with_capacity(forward_ids.len());
        let mut unique = Vec::with_capacity(forward_ids.len());
        for forward_id in forward_ids {
            anyhow::ensure!(
                state.entries.contains_key(forward_id.as_str()),
                "unknown forward_id `{forward_id}`"
            );
            if seen.insert(forward_id) {
                unique.push(forward_id.clone());
            }
        }
        Ok(unique)
    }

    async fn take_close_candidates(
        &self,
        forward_ids: &[ForwardId],
    ) -> anyhow::Result<Vec<PortForwardCloseCandidate>> {
        let forward_ids = self.validated_unique_close_ids(forward_ids).await?;
        let mut state = self.state.write().await;
        let mut candidates = Vec::with_capacity(forward_ids.len());
        for forward_id in forward_ids {
            let record = state
                .entries
                .remove(forward_id.as_str())
                .ok_or_else(|| anyhow::anyhow!("unknown forward_id `{forward_id}`"))?;
            if is_reconnecting_entry(&record.entry) {
                state.reconnecting_count = state.reconnecting_count.saturating_sub(1);
            }
            candidates.push(PortForwardCloseCandidate { forward_id, record });
        }
        Ok(candidates)
    }

    async fn restore_close_candidates(&self, candidates: Vec<PortForwardCloseCandidate>) {
        if candidates.is_empty() {
            return;
        }
        let mut state = self.state.write().await;
        for candidate in candidates {
            state.insert_record(candidate.record);
        }
    }

    pub async fn mark_failed(&self, forward_id: &str, error: String) {
        let mut state = self.state.write().await;
        if let Some(record) = state.entries.get_mut(forward_id) {
            let before = is_reconnecting_entry(&record.entry);
            mark_entry_failed(&mut record.entry, error);
            let after = is_reconnecting_entry(&record.entry);
            adjust_reconnecting_count(&mut state.reconnecting_count, before, after);
        }
    }

    pub async fn update_entry(&self, forward_id: &str, update: impl FnOnce(&mut ForwardPortEntry)) {
        let mut state = self.state.write().await;
        if let Some(record) = state.entries.get_mut(forward_id) {
            let before = is_reconnecting_entry(&record.entry);
            update(&mut record.entry);
            let after = is_reconnecting_entry(&record.entry);
            adjust_reconnecting_count(&mut state.reconnecting_count, before, after);
        }
    }

    pub async fn mark_reconnecting(
        &self,
        forward_id: &str,
        role: ForwardPortSideRole,
        error: String,
        max_reconnecting_forwards: usize,
    ) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        ensure_reconnect_capacity(&mut state, forward_id, max_reconnecting_forwards)?;
        if let Some(record) = state.entries.get_mut(forward_id) {
            if record.entry.status != ForwardPortStatus::Open {
                return Ok(());
            }
            let before = is_reconnecting_entry(&record.entry);
            prepare_reconnect_entry(&mut record.entry);
            mark_side_reconnecting(side_state_mut(&mut record.entry, role), error);
            record.entry.phase = derive_phase(&record.entry);
            let after = is_reconnecting_entry(&record.entry);
            adjust_reconnecting_count(&mut state.reconnecting_count, before, after);
        }
        Ok(())
    }

    pub async fn mark_connect_reopening_after_listen_recovery(
        &self,
        forward_id: &str,
        error: String,
        max_reconnecting_forwards: usize,
    ) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        ensure_reconnect_capacity(&mut state, forward_id, max_reconnecting_forwards)?;
        if let Some(record) = state.entries.get_mut(forward_id) {
            if record.entry.status != ForwardPortStatus::Open {
                return Ok(());
            }
            let before = is_reconnecting_entry(&record.entry);
            prepare_reconnect_entry(&mut record.entry);
            mark_side_ready(&mut record.entry.listen_state);
            mark_side_reconnecting(&mut record.entry.connect_state, error);
            record.entry.phase = derive_phase(&record.entry);
            let after = is_reconnecting_entry(&record.entry);
            adjust_reconnecting_count(&mut state.reconnecting_count, before, after);
        }
        Ok(())
    }

    pub async fn mark_ready(&self, forward_id: &str, role: ForwardPortSideRole) {
        self.update_entry(forward_id, |entry| {
            if entry.status != ForwardPortStatus::Open {
                return;
            }
            let side = match role {
                ForwardPortSideRole::Listen => &mut entry.listen_state,
                ForwardPortSideRole::Connect => &mut entry.connect_state,
            };
            side.health = ForwardPortSideHealth::Ready;
            side.last_error = None;
            entry.phase = derive_phase(entry);
        })
        .await;
    }

    pub async fn set_forward_generation(&self, forward_id: &str, generation: u64) {
        self.update_entry(forward_id, |entry| {
            if entry.status != ForwardPortStatus::Open {
                return;
            }
            entry.listen_state.generation = generation;
            entry.connect_state.generation = generation;
        })
        .await;
    }

    pub async fn drain(&self) -> Vec<PortForwardRecord> {
        let mut state = self.state.write().await;
        state.reconnecting_count = 0;
        state.entries.drain().map(|(_, record)| record).collect()
    }
}

#[derive(Default)]
struct PortForwardStoreState {
    entries: HashMap<ForwardId, PortForwardRecord>,
    reconnecting_count: usize,
}

impl PortForwardStoreState {
    fn insert_record(&mut self, record: PortForwardRecord) {
        let forward_id = record.entry.forward_id.clone();
        let is_reconnecting = is_reconnecting_entry(&record.entry);
        if let Some(old) = self.entries.insert(forward_id, record) {
            if is_reconnecting_entry(&old.entry) {
                self.reconnecting_count = self.reconnecting_count.saturating_sub(1);
            }
        }
        if is_reconnecting {
            self.reconnecting_count += 1;
        }
    }
}

fn ensure_reconnect_capacity(
    state: &mut PortForwardStoreState,
    forward_id: &str,
    max_reconnecting_forwards: usize,
) -> anyhow::Result<()> {
    let Some(record) = state.entries.get(forward_id) else {
        return Ok(());
    };
    if record.entry.status != ForwardPortStatus::Open {
        return Ok(());
    }

    let already_reconnecting = is_reconnecting_entry(&record.entry);
    if !already_reconnecting && state.reconnecting_count >= max_reconnecting_forwards {
        if let Some(record) = state.entries.get_mut(forward_id) {
            mark_entry_failed(&mut record.entry, RECONNECT_LIMIT_EXCEEDED.to_string());
        }
        return Err(anyhow::anyhow!(RECONNECT_LIMIT_EXCEEDED));
    }
    Ok(())
}

fn prepare_reconnect_entry(entry: &mut ForwardPortEntry) {
    entry.reconnect_attempts += 1;
    entry.last_reconnect_at = Some(Timestamp::now());
}

fn side_state_mut(
    entry: &mut ForwardPortEntry,
    role: ForwardPortSideRole,
) -> &mut ForwardPortSideState {
    match role {
        ForwardPortSideRole::Listen => &mut entry.listen_state,
        ForwardPortSideRole::Connect => &mut entry.connect_state,
    }
}

fn mark_side_reconnecting(side: &mut ForwardPortSideState, error: String) {
    side.health = ForwardPortSideHealth::Reconnecting;
    side.last_error = Some(error);
}

fn mark_side_ready(side: &mut ForwardPortSideState) {
    side.health = ForwardPortSideHealth::Ready;
    side.last_error = None;
}

fn is_reconnecting_entry(entry: &ForwardPortEntry) -> bool {
    entry.status == ForwardPortStatus::Open && derive_phase(entry) == ForwardPortPhase::Reconnecting
}

fn adjust_reconnecting_count(count: &mut usize, before: bool, after: bool) {
    match (before, after) {
        (false, true) => *count += 1,
        (true, false) => *count = count.saturating_sub(1),
        _ => {}
    }
}

pub struct PortForwardFilter {
    pub listen_side: Option<String>,
    pub connect_side: Option<String>,
    pub forward_ids: Vec<ForwardId>,
}

impl PortForwardFilter {
    fn matches(&self, entry: &ForwardPortEntry) -> bool {
        if let Some(listen_side) = &self.listen_side {
            if &entry.listen_side != listen_side {
                return false;
            }
        }
        if let Some(connect_side) = &self.connect_side {
            if &entry.connect_side != connect_side {
                return false;
            }
        }
        self.forward_ids.is_empty() || self.forward_ids.contains(&entry.forward_id)
    }
}

pub struct PortForwardRecord {
    pub entry: ForwardPortEntry,
    close_handle: Arc<PortForwardCloseHandle>,
}

impl PortForwardRecord {
    pub(super) fn new(
        entry: ForwardPortEntry,
        listen_session: Arc<ListenSessionControl>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            entry,
            close_handle: Arc::new(PortForwardCloseHandle::new(listen_session, cancel)),
        }
    }

    pub(super) async fn set_task(&self, task: JoinHandle<()>) {
        self.close_handle.set_task(task).await;
    }
}

struct PortForwardCloseCandidate {
    forward_id: ForwardId,
    record: PortForwardRecord,
}

struct PortForwardCloseHandle {
    listen_session: Arc<ListenSessionControl>,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl PortForwardCloseHandle {
    fn new(listen_session: Arc<ListenSessionControl>, cancel: CancellationToken) -> Self {
        Self {
            listen_session,
            cancel,
            task: Mutex::new(None),
        }
    }

    async fn set_task(&self, task: JoinHandle<()>) {
        *self.task.lock().await = Some(task);
    }

    async fn take_task(&self) -> Option<JoinHandle<()>> {
        self.task.lock().await.take()
    }
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    let PortForwardRecord {
        entry,
        close_handle: handle,
    } = record;
    let result = close_handle(&handle).await;
    if let Err(err) = result {
        tracing::warn!(
            forward_id = %entry.forward_id,
            error = %err,
            "failed to close port forward cleanly"
        );
    }
    closed_entry(entry)
}

async fn close_handle(handle: &PortForwardCloseHandle) -> anyhow::Result<()> {
    handle.cancel.cancel();
    if let Some(task) = handle.take_task().await {
        wait_for_forward_task_stop(task).await?;
    }
    close_listen_session(handle.listen_session.clone()).await
}

fn closed_entry(mut entry: ForwardPortEntry) -> ForwardPortEntry {
    entry.status = ForwardPortStatus::Closed;
    entry.phase = ForwardPortPhase::Closed;
    entry.listen_state.health = ForwardPortSideHealth::Closed;
    entry.connect_state.health = ForwardPortSideHealth::Closed;
    entry.last_error = None;
    entry
}

fn mark_entry_failed(entry: &mut ForwardPortEntry, error: String) {
    entry.status = ForwardPortStatus::Failed;
    entry.phase = ForwardPortPhase::Failed;
    entry.listen_state.health = ForwardPortSideHealth::Failed;
    entry.connect_state.health = ForwardPortSideHealth::Failed;
    entry.last_error = Some(error);
}

fn derive_phase(entry: &ForwardPortEntry) -> ForwardPortPhase {
    if entry.listen_state.health == ForwardPortSideHealth::Failed
        || entry.connect_state.health == ForwardPortSideHealth::Failed
    {
        ForwardPortPhase::Failed
    } else if entry.listen_state.health == ForwardPortSideHealth::Closed
        && entry.connect_state.health == ForwardPortSideHealth::Closed
    {
        ForwardPortPhase::Closed
    } else if entry.listen_state.health == ForwardPortSideHealth::Ready
        && entry.connect_state.health == ForwardPortSideHealth::Ready
    {
        ForwardPortPhase::Ready
    } else {
        ForwardPortPhase::Reconnecting
    }
}

pub async fn close_all(store: &PortForwardStore) {
    for record in store.drain().await {
        let _ = close_record(record).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::public::{
        ForwardPortEntry, ForwardPortLimitSummary, ForwardPortPhase, ForwardPortProtocol,
        ForwardPortSideHealth, ForwardPortSideRole, ForwardPortStatus,
    };
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::port_forward::SideHandle;
    use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;

    #[tokio::test]
    async fn mark_ready_keeps_forward_reconnecting_until_both_sides_ready() {
        let store = PortForwardStore::default();
        store.insert(test_record("fwd_state")).await;

        store
            .mark_reconnecting(
                "fwd_state",
                ForwardPortSideRole::Connect,
                "connect-side tunnel lost".to_string(),
                16,
            )
            .await
            .unwrap();
        store
            .mark_ready("fwd_state", ForwardPortSideRole::Listen)
            .await;

        let reconnecting = store.list(&filter_one("fwd_state")).await.remove(0);
        assert_eq!(reconnecting.status, ForwardPortStatus::Open);
        assert_eq!(reconnecting.phase, ForwardPortPhase::Reconnecting);
        assert_eq!(
            reconnecting.listen_state.health,
            ForwardPortSideHealth::Ready
        );
        assert_eq!(
            reconnecting.connect_state.health,
            ForwardPortSideHealth::Reconnecting
        );

        store
            .mark_ready("fwd_state", ForwardPortSideRole::Connect)
            .await;

        let ready = store.list(&filter_one("fwd_state")).await.remove(0);
        assert_eq!(ready.status, ForwardPortStatus::Open);
        assert_eq!(ready.phase, ForwardPortPhase::Ready);
        assert_eq!(ready.listen_state.health, ForwardPortSideHealth::Ready);
        assert_eq!(ready.connect_state.health, ForwardPortSideHealth::Ready);
    }

    #[tokio::test]
    async fn mark_connect_reopening_after_listen_recovery_is_atomic() {
        let store = PortForwardStore::default();
        store.insert(test_record("fwd_staged")).await;

        store
            .mark_reconnecting(
                "fwd_staged",
                ForwardPortSideRole::Listen,
                "listen-side tunnel lost".to_string(),
                16,
            )
            .await
            .unwrap();
        store
            .mark_connect_reopening_after_listen_recovery(
                "fwd_staged",
                "connect-side tunnel reopening after listen-side recovery".to_string(),
                16,
            )
            .await
            .unwrap();

        let reconnecting = store.list(&filter_one("fwd_staged")).await.remove(0);
        assert_eq!(reconnecting.status, ForwardPortStatus::Open);
        assert_eq!(reconnecting.phase, ForwardPortPhase::Reconnecting);
        assert_eq!(
            reconnecting.listen_state.health,
            ForwardPortSideHealth::Ready
        );
        assert_eq!(reconnecting.listen_state.last_error, None);
        assert_eq!(
            reconnecting.connect_state.health,
            ForwardPortSideHealth::Reconnecting
        );
        assert_eq!(
            reconnecting.connect_state.last_error.as_deref(),
            Some("connect-side tunnel reopening after listen-side recovery")
        );
    }

    #[tokio::test]
    async fn mark_ready_releases_reconnecting_capacity() {
        let store = PortForwardStore::default();
        store.insert(test_record("fwd_ready_first")).await;
        store.insert(test_record("fwd_ready_second")).await;

        store
            .mark_reconnecting(
                "fwd_ready_first",
                ForwardPortSideRole::Connect,
                "connect-side tunnel lost".to_string(),
                1,
            )
            .await
            .unwrap();
        store
            .mark_ready("fwd_ready_first", ForwardPortSideRole::Connect)
            .await;

        store
            .mark_reconnecting(
                "fwd_ready_second",
                ForwardPortSideRole::Listen,
                "listen-side tunnel lost".to_string(),
                1,
            )
            .await
            .unwrap();

        let second = store.list(&filter_one("fwd_ready_second")).await.remove(0);
        assert_eq!(second.status, ForwardPortStatus::Open);
        assert_eq!(second.phase, ForwardPortPhase::Reconnecting);
    }

    #[tokio::test]
    async fn mark_reconnecting_fails_new_forward_when_reconnect_limit_is_reached() {
        let store = PortForwardStore::default();
        store.insert(test_record("fwd_first")).await;
        store.insert(test_record("fwd_second")).await;

        store
            .mark_reconnecting(
                "fwd_first",
                ForwardPortSideRole::Connect,
                "connect-side tunnel lost".to_string(),
                1,
            )
            .await
            .unwrap();
        let error = store
            .mark_reconnecting(
                "fwd_second",
                ForwardPortSideRole::Listen,
                "listen-side tunnel lost".to_string(),
                1,
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("port_forward_limit_exceeded"));
        let first = store.list(&filter_one("fwd_first")).await.remove(0);
        assert_eq!(first.status, ForwardPortStatus::Open);
        assert_eq!(first.phase, ForwardPortPhase::Reconnecting);

        let second = store.list(&filter_one("fwd_second")).await.remove(0);
        assert_eq!(second.status, ForwardPortStatus::Failed);
        assert_eq!(second.phase, ForwardPortPhase::Failed);
        assert_eq!(
            second.last_error.as_deref(),
            Some("port_forward_limit_exceeded: broker reconnecting forward limit reached")
        );
    }

    #[tokio::test]
    async fn forward_task_handle_is_consumed_once() {
        let record = test_record("fwd_task");
        let task = tokio::spawn(async {});
        record.set_task(task).await;

        let first = record.close_handle.take_task().await;
        assert!(first.is_some());
        if let Some(task) = first {
            task.await.unwrap();
        }

        assert!(record.close_handle.take_task().await.is_none());
    }

    #[tokio::test]
    async fn close_does_not_block_unrelated_forwards_behind_waiting_close_task() {
        let store = PortForwardStore::default();
        let blocked = test_record("fwd_blocked");
        let ready = test_record("fwd_ready");
        let (release_tx, release_rx) = oneshot::channel();

        blocked
            .set_task(tokio::spawn(async move {
                let _ = release_rx.await;
            }))
            .await;
        store.insert(blocked).await;
        store.insert(ready).await;

        let blocked_close = tokio::spawn({
            let store = store.clone();
            async move { store.close(&[ForwardId::new("fwd_blocked")]).await }
        });
        tokio::task::yield_now().await;

        let ready_close = tokio::time::timeout(Duration::from_millis(200), async {
            store.close(&[ForwardId::new("fwd_ready")]).await
        })
        .await;
        assert!(
            ready_close.is_ok(),
            "unrelated close should not wait for a blocked forward task to stop"
        );
        assert!(ready_close.unwrap().is_ok());

        release_tx.send(()).unwrap();
        assert!(blocked_close.await.unwrap().is_ok());
    }

    fn filter_one(forward_id: &str) -> PortForwardFilter {
        PortForwardFilter {
            listen_side: None,
            connect_side: None,
            forward_ids: vec![ForwardId::new(forward_id)],
        }
    }

    fn test_record(forward_id: &str) -> PortForwardRecord {
        PortForwardRecord::new(
            ForwardPortEntry::new_open(
                ForwardId::new(forward_id),
                "local".to_string(),
                "127.0.0.1:10000".to_string(),
                DEFAULT_TEST_TARGET.to_string(),
                "127.0.0.1:10001".to_string(),
                ForwardPortProtocol::Tcp,
                ForwardPortLimitSummary {
                    max_active_tcp_streams: 256,
                    max_udp_peers: 256,
                    max_pending_tcp_bytes_per_stream: 256 * 1024,
                    max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
                    max_tunnel_queued_bytes:
                        remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES,
                    max_reconnecting_forwards: 16,
                },
            ),
            Arc::new(ListenSessionControl::new_for_test(
                SideHandle::local().unwrap(),
                ForwardId::new(forward_id),
                format!("session-{forward_id}"),
                ForwardPortProtocol::Tcp,
                Duration::from_secs(5),
                remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES as usize,
                None,
            )),
            CancellationToken::new(),
        )
    }
}
