use std::collections::HashMap;
use std::sync::Arc;

use remote_exec_proto::public::{ForwardPortEntry, ForwardPortStatus};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::supervisor::{ListenSessionControl, close_listen_session};

#[derive(Clone, Default)]
pub struct PortForwardStore {
    entries: Arc<RwLock<HashMap<String, PortForwardRecord>>>,
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

    pub async fn close(&self, forward_ids: &[String]) -> anyhow::Result<Vec<PortForwardRecord>> {
        let mut entries = self.entries.write().await;
        for forward_id in forward_ids {
            anyhow::ensure!(
                entries.contains_key(forward_id),
                "unknown forward_id `{forward_id}`"
            );
        }
        Ok(forward_ids
            .iter()
            .filter_map(|forward_id| entries.remove(forward_id))
            .collect())
    }

    pub async fn mark_failed(&self, forward_id: &str, error: String) {
        let mut entries = self.entries.write().await;
        if let Some(record) = entries.get_mut(forward_id) {
            record.entry.status = ForwardPortStatus::Failed;
            record.entry.last_error = Some(error);
        }
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
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    record.cancel.cancel();
    let _ = close_listen_session(record.listen_session.clone()).await;
    let mut entry = record.entry;
    entry.status = ForwardPortStatus::Closed;
    entry.last_error = None;
    entry
}

pub async fn close_all(store: &PortForwardStore) {
    for record in store.drain().await {
        let _ = close_record(record).await;
    }
}
