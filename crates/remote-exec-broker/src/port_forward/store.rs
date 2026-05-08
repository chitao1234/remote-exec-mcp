use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortPhase, ForwardPortSideHealth, ForwardPortSideRole,
    ForwardPortStatus,
};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::supervisor::{ListenSessionControl, close_listen_session, wait_for_forward_task_stop};

#[derive(Clone, Default)]
pub struct PortForwardStore {
    entries: Arc<RwLock<HashMap<String, PortForwardRecord>>>,
    close_lock: Arc<Mutex<()>>,
}

impl PortForwardStore {
    pub async fn insert(&self, record: PortForwardRecord) {
        self.entries
            .write()
            .await
            .insert(record.entry.forward_id.clone(), record);
    }

    pub async fn list(&self, filter: &PortForwardFilter) -> Vec<ForwardPortEntry> {
        let mut entries = self
            .entries
            .read()
            .await
            .values()
            .filter(|record| filter.matches(&record.entry))
            .map(|record| record.entry.clone())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.forward_id.cmp(&right.forward_id));
        entries
    }

    pub async fn open_count(&self) -> usize {
        self.entries.read().await.len()
    }

    pub async fn side_pair_count(&self, listen_side: &str, connect_side: &str) -> usize {
        self.entries
            .read()
            .await
            .values()
            .filter(|record| {
                record.entry.listen_side == listen_side && record.entry.connect_side == connect_side
            })
            .count()
    }

    pub async fn close(&self, forward_ids: &[String]) -> anyhow::Result<Vec<ForwardPortEntry>> {
        let _close_guard = self.close_lock.lock().await;
        let forward_ids = self.validated_unique_close_ids(forward_ids).await?;
        let mut closed = Vec::with_capacity(forward_ids.len());
        for forward_id in &forward_ids {
            let candidate = self.close_candidate(forward_id).await?;
            if let Err(err) = close_handle(&candidate.handle).await {
                let error = format!("closing port forward `{}`: {err:#}", candidate.forward_id);
                self.mark_failed(&candidate.forward_id, error.clone()).await;
                return Err(anyhow::anyhow!(error));
            }
            closed.push(self.remove_closed_candidate(candidate).await);
        }
        Ok(closed)
    }

    async fn validated_unique_close_ids(
        &self,
        forward_ids: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let entries = self.entries.read().await;
        let mut seen = HashSet::with_capacity(forward_ids.len());
        let mut unique = Vec::with_capacity(forward_ids.len());
        for forward_id in forward_ids {
            anyhow::ensure!(
                entries.contains_key(forward_id),
                "unknown forward_id `{forward_id}`"
            );
            if seen.insert(forward_id) {
                unique.push(forward_id.clone());
            }
        }
        Ok(unique)
    }

    async fn close_candidate(&self, forward_id: &str) -> anyhow::Result<PortForwardCloseCandidate> {
        let entries = self.entries.read().await;
        let record = entries
            .get(forward_id)
            .ok_or_else(|| anyhow::anyhow!("unknown forward_id `{forward_id}`"))?;
        Ok(PortForwardCloseCandidate {
            forward_id: forward_id.to_string(),
            handle: PortForwardCloseHandle::from(record),
            record: PortForwardRecord {
                entry: record.entry.clone(),
                listen_session: record.listen_session.clone(),
                cancel: record.cancel.clone(),
                task_done: record.task_done.clone(),
            },
        })
    }

    async fn remove_closed_candidate(
        &self,
        candidate: PortForwardCloseCandidate,
    ) -> ForwardPortEntry {
        let record = self
            .entries
            .write()
            .await
            .remove(&candidate.forward_id)
            .unwrap_or(candidate.record);
        let mut entry = record.entry;
        entry.status = ForwardPortStatus::Closed;
        entry.phase = ForwardPortPhase::Closed;
        entry.listen_state.health = ForwardPortSideHealth::Closed;
        entry.connect_state.health = ForwardPortSideHealth::Closed;
        entry.last_error = None;
        entry
    }

    pub async fn mark_failed(&self, forward_id: &str, error: String) {
        let mut entries = self.entries.write().await;
        if let Some(record) = entries.get_mut(forward_id) {
            record.entry.status = ForwardPortStatus::Failed;
            record.entry.phase = ForwardPortPhase::Failed;
            record.entry.listen_state.health = ForwardPortSideHealth::Failed;
            record.entry.connect_state.health = ForwardPortSideHealth::Failed;
            record.entry.last_error = Some(error);
        }
    }

    pub async fn update_entry(&self, forward_id: &str, update: impl FnOnce(&mut ForwardPortEntry)) {
        let mut entries = self.entries.write().await;
        if let Some(record) = entries.get_mut(forward_id) {
            update(&mut record.entry);
        }
    }

    pub async fn mark_reconnecting(
        &self,
        forward_id: &str,
        role: ForwardPortSideRole,
        error: String,
    ) {
        self.update_entry(forward_id, |entry| {
            entry.phase = ForwardPortPhase::Reconnecting;
            entry.reconnect_attempts += 1;
            entry.last_reconnect_at = Some(unix_timestamp_string());
            let side = match role {
                ForwardPortSideRole::Listen => &mut entry.listen_state,
                ForwardPortSideRole::Connect => &mut entry.connect_state,
            };
            side.health = ForwardPortSideHealth::Reconnecting;
            side.last_error = Some(error);
        })
        .await;
    }

    pub async fn mark_ready(&self, forward_id: &str, role: ForwardPortSideRole) {
        self.update_entry(forward_id, |entry| {
            entry.phase = ForwardPortPhase::Ready;
            let side = match role {
                ForwardPortSideRole::Listen => &mut entry.listen_state,
                ForwardPortSideRole::Connect => &mut entry.connect_state,
            };
            side.health = ForwardPortSideHealth::Ready;
            side.last_error = None;
        })
        .await;
    }

    pub async fn drain(&self) -> Vec<PortForwardRecord> {
        self.entries
            .write()
            .await
            .drain()
            .map(|(_, record)| record)
            .collect()
    }
}

fn unix_timestamp_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

pub struct PortForwardFilter {
    pub listen_side: Option<String>,
    pub connect_side: Option<String>,
    pub forward_ids: Vec<String>,
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
    pub(super) listen_session: Arc<ListenSessionControl>,
    pub cancel: CancellationToken,
    pub(super) task_done: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct PortForwardCloseCandidate {
    forward_id: String,
    handle: PortForwardCloseHandle,
    record: PortForwardRecord,
}

struct PortForwardCloseHandle {
    listen_session: Arc<ListenSessionControl>,
    cancel: CancellationToken,
    task_done: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl From<&PortForwardRecord> for PortForwardCloseHandle {
    fn from(record: &PortForwardRecord) -> Self {
        Self {
            listen_session: record.listen_session.clone(),
            cancel: record.cancel.clone(),
            task_done: record.task_done.clone(),
        }
    }
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    let result = close_handle(&PortForwardCloseHandle::from(&record)).await;
    if let Err(err) = result {
        tracing::warn!(
            forward_id = %record.entry.forward_id,
            error = %err,
            "failed to close port forward cleanly"
        );
    }
    closed_entry(record.entry)
}

async fn close_handle(handle: &PortForwardCloseHandle) -> anyhow::Result<()> {
    handle.cancel.cancel();
    if let Some(task) = handle.task_done.lock().await.take() {
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

pub async fn close_all(store: &PortForwardStore) {
    for record in store.drain().await {
        let _ = close_record(record).await;
    }
}
