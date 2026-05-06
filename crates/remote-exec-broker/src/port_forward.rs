use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_forward::{
    ensure_nonzero_connect_endpoint, normalize_endpoint, udp_connector_endpoint,
};
use remote_exec_proto::port_tunnel::{Frame, read_frame, write_frame, write_preface};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortProtocol as PublicForwardPortProtocol, ForwardPortSpec,
    ForwardPortStatus,
};
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortForwardLease, PortForwardProtocol as RpcPortForwardProtocol, PortLeaseRenewRequest,
    PortListenAcceptRequest, PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest,
    PortListenResponse, PortUdpDatagramReadRequest, PortUdpDatagramReadResponse,
    PortUdpDatagramWriteRequest,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::TargetHandle;
use crate::daemon_client::DaemonClientError;
use crate::local_port_backend::LocalPortClient;

const PORT_BIND_CLOSED_CODE: &str = "port_bind_closed";
const PORT_CONNECTION_CLOSED_CODE: &str = "port_connection_closed";
const FORWARD_LEASE_TTL_MS: u64 = 4_000;
const FORWARD_LEASE_RENEW_INTERVAL_MS: u64 = 1_000;

#[derive(Clone)]
pub enum SideHandle {
    Target { name: String, handle: TargetHandle },
    Local(LocalPortClient),
}

impl SideHandle {
    pub fn local() -> Self {
        Self::Local(LocalPortClient::global())
    }

    pub fn target(name: String, handle: TargetHandle) -> Self {
        Self::Target { name, handle }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Target { name, .. } => name,
            Self::Local(_) => "local",
        }
    }

    pub async fn port_listen(
        &self,
        req: &PortListenRequest,
    ) -> Result<PortListenResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_listen(req).await,
            Self::Local(client) => client.port_listen(req).await,
        }
    }

    pub async fn port_listen_accept(
        &self,
        req: &PortListenAcceptRequest,
    ) -> Result<PortListenAcceptResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_listen_accept(req).await,
            Self::Local(client) => client.port_listen_accept(req).await,
        }
    }

    pub async fn port_listen_close(
        &self,
        req: &PortListenCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_listen_close(req).await,
            Self::Local(client) => client.port_listen_close(req).await,
        }
    }

    pub async fn port_lease_renew(
        &self,
        req: &PortLeaseRenewRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_lease_renew(req).await,
            Self::Local(client) => client.port_lease_renew(req).await,
        }
    }

    pub async fn port_connect(
        &self,
        req: &PortConnectRequest,
    ) -> Result<PortConnectResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_connect(req).await,
            Self::Local(client) => client.port_connect(req).await,
        }
    }

    pub async fn port_connection_read(
        &self,
        req: &PortConnectionReadRequest,
    ) -> Result<PortConnectionReadResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_connection_read(req).await,
            Self::Local(client) => client.port_connection_read(req).await,
        }
    }

    pub async fn port_connection_write(
        &self,
        req: &PortConnectionWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_connection_write(req).await,
            Self::Local(client) => client.port_connection_write(req).await,
        }
    }

    pub async fn port_connection_close(
        &self,
        req: &PortConnectionCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_connection_close(req).await,
            Self::Local(client) => client.port_connection_close(req).await,
        }
    }

    pub async fn port_udp_datagram_read(
        &self,
        req: &PortUdpDatagramReadRequest,
    ) -> Result<PortUdpDatagramReadResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_udp_datagram_read(req).await,
            Self::Local(client) => client.port_udp_datagram_read(req).await,
        }
    }

    pub async fn port_udp_datagram_write(
        &self,
        req: &PortUdpDatagramWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_udp_datagram_write(req).await,
            Self::Local(client) => client.port_udp_datagram_write(req).await,
        }
    }

    pub async fn port_tunnel(&self) -> Result<PortTunnel, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_tunnel().await,
            Self::Local(client) => PortTunnel::local(client.state()).await,
        }
    }
}

pub struct PortTunnel {
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<anyhow::Result<Frame>>>,
}

impl PortTunnel {
    pub fn from_stream<S>(stream: S) -> Result<Self, DaemonClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (tx, mut write_rx) = mpsc::channel::<Frame>(128);
        let (read_tx, read_rx) = mpsc::channel::<anyhow::Result<Frame>>(128);
        tokio::spawn(async move {
            while let Some(frame) = write_rx.recv().await {
                if let Err(err) = write_frame(&mut writer, &frame).await {
                    tracing::debug!(error = %err, "port tunnel writer stopped");
                    return;
                }
            }
        });
        tokio::spawn(async move {
            loop {
                let frame = read_frame(&mut reader).await.map_err(anyhow::Error::from);
                let stop = frame.is_err();
                if read_tx.send(frame).await.is_err() || stop {
                    return;
                }
            }
        });
        Ok(Self {
            tx,
            rx: Mutex::new(read_rx),
        })
    }

    pub async fn local(
        state: Arc<remote_exec_host::HostRuntimeState>,
    ) -> Result<Self, DaemonClientError> {
        let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
        tokio::spawn(remote_exec_host::port_forward::serve_tunnel(
            state,
            daemon_side,
        ));
        write_preface(&mut broker_side)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Self::from_stream(broker_side)
    }

    pub async fn send(&self, frame: Frame) -> anyhow::Result<()> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("port tunnel writer is closed"))
    }

    pub async fn recv(&self) -> anyhow::Result<Frame> {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("port tunnel reader is closed"))?
    }

    pub async fn close_stream(&self, stream_id: u32) -> anyhow::Result<()> {
        self.send(Frame {
            frame_type: remote_exec_proto::port_tunnel::FrameType::Close,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
    }
}

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
    pub bind_id: String,
    pub connect_bind_id: Option<String>,
    pub lease_ids: Vec<LeaseOwner>,
    pub connect_side: SideHandle,
    pub listen_side: SideHandle,
    pub cancel: CancellationToken,
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
}

#[derive(Clone)]
struct ForwardRuntime {
    forward_id: String,
    listen_side: SideHandle,
    connect_side: SideHandle,
    protocol: RpcPortForwardProtocol,
    bind_id: String,
    connect_bind_id: Option<String>,
    connect_endpoint: String,
    lease_ids: Vec<LeaseOwner>,
    cancel: CancellationToken,
}

#[derive(Clone)]
pub struct LeaseOwner {
    side: SideHandle,
    lease_id: String,
}

pub async fn open_forward(
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    spec: &ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_endpoint = normalize_endpoint(&spec.listen_endpoint)?;
    let connect_endpoint = ensure_nonzero_connect_endpoint(&spec.connect_endpoint)?;
    let protocol = rpc_protocol(spec.protocol);
    let listen_lease_id = format!("lease_{}", uuid::Uuid::new_v4().simple());
    let mut lease_ids = vec![LeaseOwner {
        side: listen_side.clone(),
        lease_id: listen_lease_id.clone(),
    }];
    if spec.protocol == PublicForwardPortProtocol::Udp {
        validate_udp_connector_endpoint(&connect_endpoint)?;
    }
    let connect_bind_id = if spec.protocol == PublicForwardPortProtocol::Udp {
        let connector_lease_id = format!("lease_{}", uuid::Uuid::new_v4().simple());
        let bind_id =
            open_udp_connector(&connect_side, &connect_endpoint, connector_lease_id.clone())
                .await?;
        lease_ids.push(LeaseOwner {
            side: connect_side.clone(),
            lease_id: connector_lease_id,
        });
        Some(bind_id)
    } else {
        None
    };

    let listen_response = match listen_side
        .port_listen(&PortListenRequest {
            endpoint: listen_endpoint.clone(),
            protocol: protocol.clone(),
            lease: Some(port_forward_lease(listen_lease_id)),
        })
        .await
        .with_context(|| {
            format!(
                "opening {} listener on `{}` at `{listen_endpoint}`",
                format_protocol(spec.protocol),
                listen_side.name()
            )
        }) {
        Ok(response) => response,
        Err(err) => {
            if let Some(connect_bind_id) = connect_bind_id {
                let _ = connect_side
                    .port_listen_close(&PortListenCloseRequest {
                        bind_id: connect_bind_id,
                    })
                    .await;
            }
            return Err(err);
        }
    };

    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: protocol.clone(),
        bind_id: listen_response.bind_id.clone(),
        connect_bind_id: connect_bind_id.clone(),
        connect_endpoint: connect_endpoint.clone(),
        lease_ids: lease_ids.clone(),
        cancel: cancel.clone(),
    };
    spawn_forward(runtime, store);

    let entry = ForwardPortEntry {
        forward_id,
        listen_side: listen_side.name().to_string(),
        listen_endpoint: listen_response.endpoint.clone(),
        connect_side: connect_side.name().to_string(),
        connect_endpoint,
        protocol: spec.protocol,
        status: ForwardPortStatus::Open,
        last_error: None,
    };

    Ok(OpenedForward {
        record: PortForwardRecord {
            entry,
            bind_id: listen_response.bind_id,
            connect_bind_id,
            lease_ids,
            connect_side,
            listen_side,
            cancel,
        },
    })
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    record.cancel.cancel();
    let _ = record
        .listen_side
        .port_listen_close(&PortListenCloseRequest {
            bind_id: record.bind_id,
        })
        .await;
    if let Some(connect_bind_id) = record.connect_bind_id {
        let _ = record
            .connect_side
            .port_listen_close(&PortListenCloseRequest {
                bind_id: connect_bind_id,
            })
            .await;
    }
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

fn spawn_forward(runtime: ForwardRuntime, store: PortForwardStore) {
    tokio::spawn(async move {
        spawn_lease_renewer(runtime.lease_ids.clone(), runtime.cancel.clone());
        let result = match runtime.protocol {
            RpcPortForwardProtocol::Tcp => run_tcp_forward(runtime.clone()).await,
            RpcPortForwardProtocol::Udp => run_udp_forward(runtime.clone()).await,
        };
        if let Err(err) = result {
            if is_expected_close_interruption(&err) {
                tracing::debug!(
                    forward_id = %runtime.forward_id,
                    listen_side = %runtime.listen_side.name(),
                    connect_side = %runtime.connect_side.name(),
                    error = %err,
                    "port forward task stopped during close"
                );
                return;
            }
            runtime.cancel.cancel();
            store
                .mark_failed(&runtime.forward_id, err.to_string())
                .await;
            tracing::warn!(
                forward_id = %runtime.forward_id,
                listen_side = %runtime.listen_side.name(),
                connect_side = %runtime.connect_side.name(),
                error = %err,
                "port forward task stopped"
            );
        }
    });
}

async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(()),
            accepted = async {
                runtime.listen_side.port_listen_accept(&PortListenAcceptRequest {
                    bind_id: runtime.bind_id.clone(),
                }).await
            } => {
                let accepted = accepted.with_context(|| {
                    format!(
                        "accepting tcp connection on `{}` bind `{}`",
                        runtime.listen_side.name(),
                        runtime.bind_id
                    )
                })?;
                let connection_lease_id = format!("lease_{}", uuid::Uuid::new_v4().simple());
                let connection_lease = port_forward_lease(connection_lease_id.clone());
                let connection_lease_owner = LeaseOwner {
                    side: runtime.connect_side.clone(),
                    lease_id: connection_lease_id,
                };
                let connect_response = match runtime.connect_side.port_connect(&PortConnectRequest {
                    endpoint: runtime.connect_endpoint.clone(),
                    protocol: RpcPortForwardProtocol::Tcp,
                    lease: Some(connection_lease),
                }).await {
                    Ok(response) => response,
                    Err(err) => {
                        let _ = runtime.listen_side.port_connection_close(&PortConnectionCloseRequest {
                            connection_id: accepted.connection_id,
                        }).await;
                        return Err(err).context("connecting tcp forward destination");
                    }
                };
                spawn_tcp_connection_pumps(
                    runtime.forward_id.clone(),
                    runtime.listen_side.clone(),
                    accepted.connection_id,
                    runtime.connect_side.clone(),
                    connect_response.connection_id,
                    connection_lease_owner,
                );
            }
        }
    }
}

async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let connect_bind = runtime
        .connect_bind_id
        .clone()
        .context("udp forward missing connector bind")?;
    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => {
                return Ok(());
            }
            datagram = async {
                runtime.listen_side.port_udp_datagram_read(&PortUdpDatagramReadRequest {
                    bind_id: runtime.bind_id.clone(),
                }).await
            } => {
                let datagram = datagram.context("reading udp datagram from listener")?;
                runtime.connect_side.port_udp_datagram_write(&PortUdpDatagramWriteRequest {
                    bind_id: connect_bind.clone(),
                    peer: runtime.connect_endpoint.clone(),
                    data: datagram.data.clone(),
                }).await.context("writing udp datagram to connector")?;
                let reply = runtime.connect_side.port_udp_datagram_read(&PortUdpDatagramReadRequest {
                    bind_id: connect_bind.clone(),
                }).await.context("reading udp reply from connector")?;
                runtime.listen_side.port_udp_datagram_write(&PortUdpDatagramWriteRequest {
                    bind_id: runtime.bind_id.clone(),
                    peer: datagram.peer,
                    data: reply.data,
                }).await.context("writing udp reply to listener peer")?;
            }
        }
    }
}

async fn open_udp_connector(
    connect_side: &SideHandle,
    connect_endpoint: &str,
    lease_id: String,
) -> anyhow::Result<String> {
    let bind_endpoint = udp_connector_endpoint(connect_endpoint)?;
    let response = connect_side
        .port_listen(&PortListenRequest {
            endpoint: bind_endpoint.to_string(),
            protocol: RpcPortForwardProtocol::Udp,
            lease: Some(port_forward_lease(lease_id)),
        })
        .await
        .with_context(|| format!("opening udp connector on `{}`", connect_side.name()))?;
    Ok(response.bind_id)
}

fn spawn_tcp_connection_pumps(
    forward_id: String,
    left_side: SideHandle,
    left_connection_id: String,
    right_side: SideHandle,
    right_connection_id: String,
    lease_owner: LeaseOwner,
) {
    let cleanup_left_side = left_side.clone();
    let cleanup_right_side = right_side.clone();
    let log_left_connection_id = left_connection_id.clone();
    let log_right_connection_id = right_connection_id.clone();
    let cleanup_left_connection_id = left_connection_id.clone();
    let cleanup_right_connection_id = right_connection_id.clone();
    let left_to_right = pump_tcp_bytes(
        forward_id.clone(),
        left_side.clone(),
        left_connection_id.clone(),
        right_side.clone(),
        right_connection_id.clone(),
    );
    let right_to_left = pump_tcp_bytes(
        forward_id,
        right_side.clone(),
        right_connection_id.clone(),
        left_side.clone(),
        left_connection_id.clone(),
    );
    tokio::spawn(async move {
        let renew_cancel = CancellationToken::new();
        spawn_lease_renewer(vec![lease_owner], renew_cancel.clone());
        tokio::select! {
            result = left_to_right => {
                log_pump_result(&log_left_connection_id, &log_right_connection_id, result);
            }
            result = right_to_left => {
                log_pump_result(&log_right_connection_id, &log_left_connection_id, result);
            }
        }
        renew_cancel.cancel();
        close_connection_pair(
            cleanup_left_side,
            cleanup_left_connection_id,
            cleanup_right_side,
            cleanup_right_connection_id,
        )
        .await;
    });
}

async fn pump_tcp_bytes(
    forward_id: String,
    read_side: SideHandle,
    read_connection_id: String,
    write_side: SideHandle,
    write_connection_id: String,
) -> anyhow::Result<()> {
    loop {
        let chunk = read_side
            .port_connection_read(&PortConnectionReadRequest {
                connection_id: read_connection_id.clone(),
            })
            .await
            .with_context(|| {
                format!(
                    "reading tcp bytes for forward `{forward_id}` from `{}`",
                    read_side.name()
                )
            })?;
        if chunk.eof {
            return Ok(());
        }
        write_side
            .port_connection_write(&PortConnectionWriteRequest {
                connection_id: write_connection_id.clone(),
                data: chunk.data,
            })
            .await
            .with_context(|| {
                format!(
                    "writing tcp bytes for forward `{forward_id}` to `{}`",
                    write_side.name()
                )
            })?;
    }
}

async fn close_connection_pair(
    left_side: SideHandle,
    left_connection_id: String,
    right_side: SideHandle,
    right_connection_id: String,
) {
    let _ = left_side
        .port_connection_close(&PortConnectionCloseRequest {
            connection_id: left_connection_id.clone(),
        })
        .await;
    let _ = right_side
        .port_connection_close(&PortConnectionCloseRequest {
            connection_id: right_connection_id.clone(),
        })
        .await;
    tracing::debug!(
        left_connection_id = %left_connection_id,
        right_connection_id = %right_connection_id,
        "tcp port forward connection pair ended"
    );
}

fn spawn_lease_renewer(lease_ids: Vec<LeaseOwner>, cancel: CancellationToken) {
    if lease_ids.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(
            FORWARD_LEASE_RENEW_INTERVAL_MS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = interval.tick() => {}
            }

            for lease in &lease_ids {
                let result = lease
                    .side
                    .port_lease_renew(&PortLeaseRenewRequest {
                        lease_id: lease.lease_id.clone(),
                        ttl_ms: FORWARD_LEASE_TTL_MS,
                    })
                    .await;
                if let Err(err) = result {
                    tracing::debug!(
                        lease_id = %lease.lease_id,
                        side = %lease.side.name(),
                        error = %err,
                        "port forward lease renew stopped"
                    );
                    cancel.cancel();
                    return;
                }
            }
        }
    });
}

fn log_pump_result(from: &str, to: &str, result: anyhow::Result<()>) {
    if let Err(err) = result {
        tracing::debug!(
            from_connection_id = %from,
            to_connection_id = %to,
            error = %err,
            "tcp port forward pump stopped"
        );
    }
}

fn validate_udp_connector_endpoint(connect_endpoint: &str) -> anyhow::Result<()> {
    ensure_nonzero_connect_endpoint(connect_endpoint)?;
    Ok(())
}

fn is_expected_close_interruption(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<DaemonClientError>()
            .and_then(DaemonClientError::rpc_code)
            .is_some_and(|code| {
                code == PORT_BIND_CLOSED_CODE || code == PORT_CONNECTION_CLOSED_CODE
            })
    })
}

fn port_forward_lease(lease_id: String) -> PortForwardLease {
    PortForwardLease {
        lease_id,
        ttl_ms: FORWARD_LEASE_TTL_MS,
    }
}

fn rpc_protocol(protocol: PublicForwardPortProtocol) -> RpcPortForwardProtocol {
    match protocol {
        PublicForwardPortProtocol::Tcp => RpcPortForwardProtocol::Tcp,
        PublicForwardPortProtocol::Udp => RpcPortForwardProtocol::Udp,
    }
}

fn format_protocol(protocol: PublicForwardPortProtocol) -> &'static str {
    match protocol {
        PublicForwardPortProtocol::Tcp => "tcp",
        PublicForwardPortProtocol::Udp => "udp",
    }
}

#[cfg(test)]
mod port_tunnel_tests {
    use remote_exec_proto::port_tunnel::{Frame, FrameType};

    use super::*;

    #[tokio::test]
    async fn local_port_tunnel_binds_tcp_listener() {
        let tunnel = SideHandle::local().port_tunnel().await.unwrap();
        tunnel
            .send(Frame {
                frame_type: FrameType::TcpListen,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "endpoint": "127.0.0.1:0"
                }))
                .unwrap(),
                data: Vec::new(),
            })
            .await
            .unwrap();

        let frame = tunnel.recv().await.unwrap();

        assert_eq!(frame.frame_type, FrameType::TcpListenOk);
    }
}
