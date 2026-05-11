use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

#[derive(Debug)]
pub(super) struct UdpPeerConnector {
    pub(super) stream_id: u32,
    pub(super) last_used: Instant,
}

#[derive(Default)]
pub(super) struct UdpConnectorMap {
    inner: Mutex<UdpConnectorMapState>,
}

#[derive(Default)]
struct UdpConnectorMapState {
    connector_by_peer: HashMap<String, UdpPeerConnector>,
    peer_by_connector: HashMap<u32, String>,
}

impl UdpConnectorMap {
    pub(super) async fn get_mut_by_peer<F, T>(&self, peer: &str, op: F) -> Option<T>
    where
        F: FnOnce(&mut UdpPeerConnector) -> T,
    {
        let mut state = self.inner.lock().await;
        state.connector_by_peer.get_mut(peer).map(op)
    }

    pub(super) async fn insert(&self, peer: String, stream_id: u32, connector: UdpPeerConnector) {
        let mut state = self.inner.lock().await;
        state.peer_by_connector.insert(stream_id, peer.clone());
        state.connector_by_peer.insert(peer, connector);
    }

    pub(super) async fn peer_for_stream_id(&self, stream_id: u32) -> Option<String> {
        self.inner
            .lock()
            .await
            .peer_by_connector
            .get(&stream_id)
            .cloned()
    }

    pub(super) async fn remove_by_stream_id(&self, stream_id: u32) -> Option<UdpPeerConnector> {
        let mut state = self.inner.lock().await;
        let peer = state.peer_by_connector.remove(&stream_id)?;
        state.connector_by_peer.remove(&peer)
    }

    pub(super) async fn sweep_idle(
        &self,
        now: Instant,
        idle_timeout: Duration,
    ) -> Vec<(u32, UdpPeerConnector)> {
        let expired = {
            let state = self.inner.lock().await;
            state
                .connector_by_peer
                .iter()
                .filter_map(|(peer, connector)| {
                    (now.duration_since(connector.last_used) >= idle_timeout)
                        .then_some((peer.clone(), connector.stream_id))
                })
                .collect::<Vec<_>>()
        };

        let mut state = self.inner.lock().await;
        let mut removed = Vec::with_capacity(expired.len());
        for (peer, stream_id) in expired {
            let still_expired = state.connector_by_peer.get(&peer).is_some_and(|connector| {
                connector.stream_id == stream_id
                    && now.duration_since(connector.last_used) >= idle_timeout
            });
            if !still_expired {
                continue;
            }
            if let Some(connector) = state.connector_by_peer.remove(&peer) {
                state.peer_by_connector.remove(&stream_id);
                removed.push((stream_id, connector));
            }
        }
        removed
    }

    pub(super) async fn len(&self) -> usize {
        self.inner.lock().await.connector_by_peer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connector_map_inserts_and_updates_by_peer() {
        let map = UdpConnectorMap::default();
        let before = Instant::now() - Duration::from_secs(5);
        let after = Instant::now();

        map.insert(
            "127.0.0.1:10000".to_string(),
            3,
            UdpPeerConnector {
                stream_id: 3,
                last_used: before,
            },
        )
        .await;

        let stream_id = map
            .get_mut_by_peer("127.0.0.1:10000", |connector| {
                connector.last_used = after;
                connector.stream_id
            })
            .await;
        assert_eq!(stream_id, Some(3));
        assert_eq!(map.len().await, 1);
    }

    #[tokio::test]
    async fn connector_map_removes_by_stream_id() {
        let map = UdpConnectorMap::default();
        map.insert(
            "127.0.0.1:10000".to_string(),
            5,
            UdpPeerConnector {
                stream_id: 5,
                last_used: Instant::now(),
            },
        )
        .await;

        let removed = map.remove_by_stream_id(5).await;
        assert_eq!(removed.map(|connector| connector.stream_id), Some(5));
        assert_eq!(map.peer_for_stream_id(5).await, None);
        assert_eq!(map.len().await, 0);
    }

    #[tokio::test]
    async fn connector_map_sweeps_idle_connectors() {
        let map = UdpConnectorMap::default();
        let now = Instant::now();
        map.insert(
            "127.0.0.1:10000".to_string(),
            7,
            UdpPeerConnector {
                stream_id: 7,
                last_used: now - Duration::from_secs(61),
            },
        )
        .await;
        map.insert(
            "127.0.0.1:10001".to_string(),
            9,
            UdpPeerConnector {
                stream_id: 9,
                last_used: now,
            },
        )
        .await;

        let removed = map.sweep_idle(now, Duration::from_secs(60)).await;
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, 7);
        assert_eq!(map.peer_for_stream_id(7).await, None);
        assert_eq!(
            map.peer_for_stream_id(9).await.as_deref(),
            Some("127.0.0.1:10001")
        );
        assert_eq!(map.len().await, 1);
    }

    #[tokio::test]
    async fn connector_map_sweep_keeps_recently_refreshed_connectors() {
        let map = UdpConnectorMap::default();
        let old = Instant::now() - Duration::from_secs(120);
        let fresh = Instant::now();

        map.insert(
            "127.0.0.1:10000".to_string(),
            7,
            UdpPeerConnector {
                stream_id: 7,
                last_used: old,
            },
        )
        .await;
        map.get_mut_by_peer("127.0.0.1:10000", |connector| {
            connector.last_used = fresh;
        })
        .await;

        let removed = map.sweep_idle(fresh, Duration::from_secs(60)).await;
        assert!(removed.is_empty());
        assert_eq!(map.len().await, 1);
    }
}
