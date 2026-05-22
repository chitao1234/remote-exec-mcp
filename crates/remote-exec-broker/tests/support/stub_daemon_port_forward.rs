use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONNECTION, UPGRADE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use remote_exec_host::{
    HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig, build_runtime_state,
};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, TunnelLimitSummary,
    UPGRADE_TOKEN,
};
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};
use remote_exec_test_support::port_tunnel_io::{
    read_frame, read_preface, write_frame, write_preface,
};
#[cfg(test)]
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{ExecWriteBehavior, StubDaemonState, spawn_stub_task, stub_daemon_state};

#[derive(Clone)]
pub(super) struct StubPortForwardState {
    _tempdir: Arc<tempfile::TempDir>,
    runtime: Arc<remote_exec_host::HostRuntimeState>,
    control: Arc<Mutex<StubPortTunnelControl>>,
    notify: Arc<Notify>,
}

impl StubPortForwardState {
    pub(super) fn new(target: &str) -> Self {
        let tempdir = Arc::new(tempfile::tempdir().expect("stub port tunnel tempdir"));
        Self {
            runtime: build_stub_port_tunnel_state(target, tempdir.path()),
            _tempdir: tempdir,
            control: Arc::new(Mutex::new(StubPortTunnelControl::default())),
            notify: Arc::new(Notify::new()),
        }
    }
}

#[derive(Clone)]
enum ResumeBehavior {
    PassThrough,
    DropTransport,
    HangTransport,
    SendError { code: String, message: String },
}

#[derive(Clone, Copy)]
enum TcpConnectOkBehavior {
    PassThrough,
    DropAll,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpConnectorStats {
    pub active: usize,
    pub max_observed: usize,
    pub opened: usize,
}

struct StubPortTunnelControl {
    enabled: bool,
    resume_behavior: ResumeBehavior,
    tcp_connect_ok_behavior: TcpConnectOkBehavior,
    close_transports_on_second_session_open: bool,
    tunnel_open_count: usize,
    connect_tunnel_open_count: usize,
    current_transport_id: Option<u64>,
    resume_attempt_count: usize,
    port_tunnel_upgrades_to_fail: usize,
    connect_tunnel_opens_to_drop: usize,
    delay_session_ready_after_first: Option<Duration>,
    blocked_session_ready_after_first: Option<BlockedPortTunnelEventState>,
    blocked_connect_tunnel_open_after_first: Option<BlockedPortTunnelEventState>,
    override_session_ready_resume_timeout_ms: Option<u64>,
    override_tunnel_ready_limits: Option<TunnelLimitSummary>,
    session_ready_count: usize,
    next_transport_id: u64,
    active_transports: Vec<ActivePortTunnelTransport>,
    active_listen_transports: Vec<ActivePortTunnelTransport>,
    active_connect_transports: Vec<ActivePortTunnelTransport>,
    closed_transport_count: usize,
    active_udp_connector_streams: HashSet<u32>,
    max_observed_udp_connector_streams: usize,
    opened_udp_connector_streams: usize,
    udp_connector_bind_errors_remaining: usize,
    heartbeat_acks_to_drop: usize,
}

#[derive(Clone)]
struct ActivePortTunnelTransport {
    id: u64,
    cancel: CancellationToken,
}

struct BlockedPortTunnelEventState {
    release_rx: Option<oneshot::Receiver<()>>,
    blocked_tx: Option<oneshot::Sender<()>>,
}

impl Default for StubPortTunnelControl {
    fn default() -> Self {
        Self {
            enabled: false,
            resume_behavior: ResumeBehavior::PassThrough,
            tcp_connect_ok_behavior: TcpConnectOkBehavior::PassThrough,
            close_transports_on_second_session_open: false,
            tunnel_open_count: 0,
            connect_tunnel_open_count: 0,
            current_transport_id: None,
            resume_attempt_count: 0,
            port_tunnel_upgrades_to_fail: 0,
            connect_tunnel_opens_to_drop: 0,
            delay_session_ready_after_first: None,
            blocked_session_ready_after_first: None,
            blocked_connect_tunnel_open_after_first: None,
            override_session_ready_resume_timeout_ms: None,
            override_tunnel_ready_limits: None,
            session_ready_count: 0,
            next_transport_id: 1,
            active_transports: Vec::new(),
            active_listen_transports: Vec::new(),
            active_connect_transports: Vec::new(),
            closed_transport_count: 0,
            active_udp_connector_streams: HashSet::new(),
            max_observed_udp_connector_streams: 0,
            opened_udp_connector_streams: 0,
            udp_connector_bind_errors_remaining: 0,
            heartbeat_acks_to_drop: 0,
        }
    }
}

pub(crate) async fn enable_reconnectable_port_tunnel(state: &StubDaemonState) {
    state.port_forward.control.lock().await.enabled = true;
}

pub(crate) async fn force_close_port_tunnel_transport(state: &StubDaemonState) {
    let active_transports = {
        let mut control = state.port_forward.control.lock().await;
        control.active_listen_transports.clear();
        control.active_connect_transports.clear();
        std::mem::take(&mut control.active_transports)
    };
    for transport in active_transports {
        transport.cancel.cancel();
    }
}

pub(crate) async fn force_close_listen_port_tunnel_transport(state: &StubDaemonState) {
    let active_transports = {
        let mut control = state.port_forward.control.lock().await;
        let active_transports = std::mem::take(&mut control.active_listen_transports);
        remove_active_transport_ids(&mut control.active_transports, &active_transports);
        active_transports
    };
    for transport in active_transports {
        transport.cancel.cancel();
    }
}

pub(crate) async fn force_close_connect_port_tunnel_transport(state: &StubDaemonState) {
    let active_transports = {
        let mut control = state.port_forward.control.lock().await;
        let active_transports = std::mem::take(&mut control.active_connect_transports);
        remove_active_transport_ids(&mut control.active_transports, &active_transports);
        active_transports
    };
    for transport in active_transports {
        transport.cancel.cancel();
    }
}

fn remove_active_transport_ids(
    active_transports: &mut Vec<ActivePortTunnelTransport>,
    removed: &[ActivePortTunnelTransport],
) {
    active_transports.retain(|transport| !removed.iter().any(|removed| removed.id == transport.id));
}

pub(crate) async fn wait_for_port_tunnel_transports(state: &StubDaemonState, count: usize) {
    wait_for_port_tunnel_condition(state, |control| control.active_transports.len() >= count).await;
}

pub(crate) async fn wait_for_connect_port_tunnel_transports(state: &StubDaemonState, count: usize) {
    wait_for_port_tunnel_condition(state, |control| {
        control.active_connect_transports.len() >= count
    })
    .await;
}

pub(crate) async fn wait_for_listen_port_tunnel_transports(state: &StubDaemonState, count: usize) {
    wait_for_port_tunnel_condition(state, |control| {
        control.active_listen_transports.len() >= count
    })
    .await;
}

pub(crate) async fn closed_port_tunnel_transport_count(state: &StubDaemonState) -> usize {
    state
        .port_forward
        .control
        .lock()
        .await
        .closed_transport_count
}

pub(crate) async fn wait_for_closed_port_tunnel_transports(state: &StubDaemonState, count: usize) {
    wait_for_port_tunnel_condition(state, |control| control.closed_transport_count >= count).await;
}

pub(crate) async fn resume_attempt_count(state: &StubDaemonState) -> usize {
    state.port_forward.control.lock().await.resume_attempt_count
}

pub(crate) async fn wait_for_resume_attempts(state: &StubDaemonState, count: usize) {
    wait_for_port_tunnel_condition(state, |control| control.resume_attempt_count >= count).await;
}

async fn wait_for_port_tunnel_condition(
    state: &StubDaemonState,
    mut condition: impl FnMut(&StubPortTunnelControl) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            {
                let control = state.port_forward.control.lock().await;
                if condition(&control) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stub port tunnel condition should become true");
}

pub(crate) async fn fail_next_port_tunnel_upgrades(state: &StubDaemonState, count: usize) {
    state
        .port_forward
        .control
        .lock()
        .await
        .port_tunnel_upgrades_to_fail += count;
}

pub(crate) async fn drop_next_connect_tunnel_opens(state: &StubDaemonState, count: usize) {
    state
        .port_forward
        .control
        .lock()
        .await
        .connect_tunnel_opens_to_drop += count;
}

pub(crate) async fn block_session_resume(state: &StubDaemonState) {
    state.port_forward.control.lock().await.resume_behavior = ResumeBehavior::DropTransport;
}

pub(crate) async fn hang_session_resume(state: &StubDaemonState) {
    state.port_forward.control.lock().await.resume_behavior = ResumeBehavior::HangTransport;
}

pub(crate) async fn set_session_resume_timeout(state: &StubDaemonState, timeout: Duration) {
    state
        .port_forward
        .control
        .lock()
        .await
        .override_session_ready_resume_timeout_ms = Some(timeout.as_millis() as u64);
}

pub(crate) async fn delay_session_ready_after_first(state: &StubDaemonState, delay: Duration) {
    state
        .port_forward
        .control
        .lock()
        .await
        .delay_session_ready_after_first = Some(delay);
}

pub(crate) async fn block_session_ready_after_first(
    state: &StubDaemonState,
) -> BlockedSessionReady {
    let (release_tx, release_rx) = oneshot::channel();
    let (blocked_tx, blocked_rx) = oneshot::channel();
    state
        .port_forward
        .control
        .lock()
        .await
        .blocked_session_ready_after_first = Some(BlockedPortTunnelEventState {
        release_rx: Some(release_rx),
        blocked_tx: Some(blocked_tx),
    });
    BlockedSessionReady {
        release_tx: Some(release_tx),
        blocked_rx,
    }
}

pub(crate) struct BlockedSessionReady {
    release_tx: Option<oneshot::Sender<()>>,
    blocked_rx: oneshot::Receiver<()>,
}

impl BlockedSessionReady {
    pub(crate) async fn wait_blocked(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.blocked_rx)
            .await
            .expect("session ready frame should become blocked");
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

pub(crate) async fn block_connect_tunnel_open_after_first(
    state: &StubDaemonState,
) -> BlockedConnectTunnelOpen {
    let (release_tx, release_rx) = oneshot::channel();
    let (blocked_tx, blocked_rx) = oneshot::channel();
    state
        .port_forward
        .control
        .lock()
        .await
        .blocked_connect_tunnel_open_after_first = Some(BlockedPortTunnelEventState {
        release_rx: Some(release_rx),
        blocked_tx: Some(blocked_tx),
    });
    BlockedConnectTunnelOpen {
        release_tx: Some(release_tx),
        blocked_rx,
    }
}

pub(crate) struct BlockedConnectTunnelOpen {
    release_tx: Option<oneshot::Sender<()>>,
    blocked_rx: oneshot::Receiver<()>,
}

impl BlockedConnectTunnelOpen {
    pub(crate) async fn wait_blocked(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.blocked_rx)
            .await
            .expect("connect tunnel open frame should become blocked");
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

pub(crate) async fn override_tunnel_ready_limits(
    state: &StubDaemonState,
    limits: TunnelLimitSummary,
) {
    state
        .port_forward
        .control
        .lock()
        .await
        .override_tunnel_ready_limits = Some(limits);
}

pub(crate) async fn set_port_tunnel_resume_error(
    state: &StubDaemonState,
    code: &str,
    message: &str,
) {
    state.port_forward.control.lock().await.resume_behavior = ResumeBehavior::SendError {
        code: code.to_string(),
        message: message.to_string(),
    };
}

pub(crate) async fn drop_tcp_connect_ok_frames(state: &StubDaemonState) {
    state
        .port_forward
        .control
        .lock()
        .await
        .tcp_connect_ok_behavior = TcpConnectOkBehavior::DropAll;
}

pub(crate) async fn fail_next_udp_connector_bind(state: &StubDaemonState) {
    state
        .port_forward
        .control
        .lock()
        .await
        .udp_connector_bind_errors_remaining += 1;
}

pub(crate) async fn drop_next_port_tunnel_heartbeat_ack(state: &StubDaemonState) {
    state
        .port_forward
        .control
        .lock()
        .await
        .heartbeat_acks_to_drop += 1;
}

pub(crate) async fn fail_first_forward_runtime_before_multi_open_finishes(state: &StubDaemonState) {
    let mut control = state.port_forward.control.lock().await;
    control.close_transports_on_second_session_open = true;
    control.delay_session_ready_after_first = Some(Duration::from_millis(500));
    control.resume_behavior = ResumeBehavior::SendError {
        code: "forced_resume_failure".to_string(),
        message: "forced resume failure".to_string(),
    };
}

pub(crate) async fn udp_connector_stats(state: &StubDaemonState) -> UdpConnectorStats {
    let control = state.port_forward.control.lock().await;
    UdpConnectorStats {
        active: control.active_udp_connector_streams.len(),
        max_observed: control.max_observed_udp_connector_streams,
        opened: control.opened_udp_connector_streams,
    }
}

pub(crate) async fn tunnel_open_count(state: &StubDaemonState) -> usize {
    state.port_forward.control.lock().await.tunnel_open_count
}

#[cfg(test)]
pub(crate) async fn assert_port_tunnel_relay_preserves_partial_frame_reads() {
    let state = stub_daemon_state(
        "relay-test",
        ExecWriteBehavior::Success,
        std::env::consts::OS,
        false,
    );
    let (mut broker_peer, broker_relay) = tokio::io::duplex(4096);
    let (mut daemon_peer, daemon_relay) = tokio::io::duplex(4096);
    let cancel = CancellationToken::new();
    let relay_cancel = cancel.clone();
    let relay = tokio::spawn(async move {
        relay_port_tunnel_frames(state, broker_relay, daemon_relay, relay_cancel, None).await
    });

    let broker_frame = Frame {
        frame_type: FrameType::TcpData,
        flags: 0,
        stream_id: 0x0102_0304,
        meta: Vec::new(),
        data: b"from-broker".to_vec(),
    };
    let mut encoded_broker_frame = Vec::new();
    write_frame(&mut encoded_broker_frame, &broker_frame)
        .await
        .unwrap();
    broker_peer
        .write_all(&encoded_broker_frame[..2])
        .await
        .unwrap();
    tokio::task::yield_now().await;

    let daemon_frame = Frame {
        frame_type: FrameType::TcpData,
        flags: 0,
        stream_id: 9,
        meta: Vec::new(),
        data: b"from-daemon".to_vec(),
    };
    write_frame(&mut daemon_peer, &daemon_frame).await.unwrap();
    let forwarded_to_broker =
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut broker_peer))
            .await
            .expect("relay should forward daemon frame while broker frame is partial")
            .unwrap();
    assert_eq!(forwarded_to_broker, daemon_frame);

    broker_peer
        .write_all(&encoded_broker_frame[2..])
        .await
        .unwrap();
    let forwarded_to_daemon =
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut daemon_peer))
            .await
            .expect("relay should preserve and forward partial broker frame")
            .unwrap();
    assert_eq!(forwarded_to_daemon, broker_frame);

    cancel.cancel();
    drop(broker_peer);
    drop(daemon_peer);
    relay.await.unwrap().unwrap();
}

pub(super) async fn port_tunnel(
    State(state): State<StubDaemonState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    {
        let mut control = state.port_forward.control.lock().await;
        if !control.enabled {
            return Err(unsupported_port_tunnel_request());
        }
        if control.port_tunnel_upgrades_to_fail > 0 {
            control.port_tunnel_upgrades_to_fail -= 1;
            return Err(port_tunnel_unavailable_request(
                "forced transient port tunnel upgrade failure",
            ));
        }
    }

    validate_port_tunnel_upgrade_headers(&headers)?;
    let on_upgrade = upgrade::on(request);
    let handler_state = state.clone();

    spawn_stub_task(&state, "port-tunnel-upgrade", async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                handle_port_tunnel_upgrade(handler_state, TokioIo::new(upgraded)).await?;
            }
            Err(err) => {
                return Err(err.into());
            }
        }
        Ok(())
    })
    .await;

    Ok((
        StatusCode::SWITCHING_PROTOCOLS,
        [(CONNECTION, "Upgrade"), (UPGRADE, UPGRADE_TOKEN)],
        Body::empty(),
    )
        .into_response())
}

fn build_stub_port_tunnel_state(
    target: &str,
    tempdir: &Path,
) -> Arc<remote_exec_host::HostRuntimeState> {
    let workdir = tempdir.join("port-tunnel-workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    Arc::new(
        build_runtime_state(HostRuntimeConfig {
            target: target.to_string(),
            default_workdir: workdir,
            windows_posix_root: None,
            sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
            max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: true,
            pty: PtyMode::None,
            default_shell: None,
            yield_time: YieldTimeConfig::default(),
            port_forward_limits: remote_exec_host::HostPortForwardLimits::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: ProcessEnvironment::capture_current(),
        })
        .unwrap(),
    )
}

async fn handle_port_tunnel_upgrade<S>(state: StubDaemonState, mut stream: S) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    read_preface(&mut stream).await?;
    let first_frame = read_frame(&mut stream).await?;

    let first_frame_meta = if first_frame.frame_type == FrameType::TunnelOpen {
        serde_json::from_slice::<serde_json::Value>(&first_frame.meta).ok()
    } else {
        None
    };
    let is_resume_open = first_frame_meta
        .as_ref()
        .and_then(|meta| meta.get("resume_session_id"))
        .is_some_and(|value| !value.is_null());
    if is_resume_open {
        let resume_behavior = {
            let mut control = state.port_forward.control.lock().await;
            control.resume_attempt_count += 1;
            control.resume_behavior.clone()
        };
        state.port_forward.notify.notify_waiters();
        match resume_behavior {
            ResumeBehavior::PassThrough => {}
            ResumeBehavior::DropTransport => return Ok(()),
            ResumeBehavior::HangTransport => {
                std::future::pending::<()>().await;
                return Ok(());
            }
            ResumeBehavior::SendError { code, message } => {
                write_frame(
                    &mut stream,
                    &Frame {
                        frame_type: FrameType::Error,
                        flags: 0,
                        stream_id: 0,
                        meta: serde_json::to_vec(&serde_json::json!({
                            "code": code,
                            "message": message,
                            "fatal": false,
                        }))?,
                        data: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
        }
    }

    let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
    let tunnel_state = state.port_forward.runtime.clone();
    state
        .port_forward
        .runtime
        .spawn_background_task("stub-inner-port-tunnel", async move {
            remote_exec_host::port_forward::serve_tunnel(tunnel_state, daemon_side)
                .await
                .map_err(|err| anyhow::anyhow!("{}: {}", err.wire_code(), err.message))
        })
        .await;
    write_preface(&mut broker_side).await?;
    let cancel = CancellationToken::new();
    let observed = observe_broker_to_daemon_frame(&state, &first_frame).await?;
    for transport in observed.transports_to_cancel {
        transport.cancel.cancel();
    }
    if observed.cancel_current_transport {
        cancel.cancel();
    }
    if let Some(error) = observed.error {
        write_frame(&mut stream, &error).await?;
    }
    if observed.forward {
        write_frame(&mut broker_side, &first_frame).await?;
    }

    let transport_id = {
        let mut control = state.port_forward.control.lock().await;
        let transport_id = control.next_transport_id;
        let transport = ActivePortTunnelTransport {
            id: transport_id,
            cancel: cancel.clone(),
        };
        control.next_transport_id += 1;
        control.current_transport_id = Some(transport_id);
        control.active_transports.push(transport.clone());
        if observed.listen_role_transport {
            control.active_listen_transports.push(transport);
        } else {
            control.active_connect_transports.push(transport);
        }
        transport_id
    };
    state.port_forward.notify.notify_waiters();

    relay_port_tunnel_frames(state, stream, broker_side, cancel, Some(transport_id)).await
}

struct ObservedBrokerFrame {
    forward: bool,
    error: Option<Frame>,
    transports_to_cancel: Vec<ActivePortTunnelTransport>,
    cancel_current_transport: bool,
    listen_role_transport: bool,
    block_open: Option<(oneshot::Sender<()>, Option<oneshot::Receiver<()>>)>,
}

async fn relay_port_tunnel_frames<S1, S2>(
    state: StubDaemonState,
    external: S1,
    internal: S2,
    cancel: CancellationToken,
    transport_id: Option<u64>,
) -> anyhow::Result<()>
where
    S1: AsyncRead + AsyncWrite + Unpin,
    S2: AsyncRead + AsyncWrite + Unpin,
{
    let (external_reader, external_writer) = tokio::io::split(external);
    let (internal_reader, internal_writer) = tokio::io::split(internal);
    let (external_tx, external_rx) = mpsc::channel(128);

    let broker_to_daemon = relay_broker_to_daemon_frames(
        state.clone(),
        external_reader,
        internal_writer,
        external_tx.clone(),
        cancel.clone(),
    );
    let daemon_to_broker = relay_daemon_to_broker_frames(
        state.clone(),
        internal_reader,
        external_tx.clone(),
        cancel.clone(),
    );
    drop(external_tx);
    let external_writer = relay_external_frames(external_writer, external_rx, cancel.clone());

    let result = tokio::select! {
        result = broker_to_daemon => result,
        result = daemon_to_broker => result,
        result = external_writer => result,
    };
    cancel.cancel();
    {
        let mut control = state.port_forward.control.lock().await;
        if let Some(transport_id) = transport_id {
            control
                .active_transports
                .retain(|transport| transport.id != transport_id);
            control
                .active_listen_transports
                .retain(|transport| transport.id != transport_id);
            control
                .active_connect_transports
                .retain(|transport| transport.id != transport_id);
            if control.current_transport_id == Some(transport_id) {
                control.current_transport_id = None;
            }
        }
        control.closed_transport_count += 1;
    }
    state.port_forward.notify.notify_waiters();
    result
}

async fn relay_broker_to_daemon_frames<R, W>(
    state: StubDaemonState,
    mut external: R,
    mut internal: W,
    external_tx: mpsc::Sender<Frame>,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let Some(frame) = read_relay_frame(&mut external, &cancel).await? else {
            return Ok(());
        };
        let observed = observe_broker_to_daemon_frame(&state, &frame).await?;
        for transport in observed.transports_to_cancel {
            transport.cancel.cancel();
        }
        if observed.cancel_current_transport {
            cancel.cancel();
        }
        if let Some(error) = observed.error {
            if external_tx.send(error).await.is_err() {
                return Ok(());
            }
        }
        if observed.forward {
            write_frame(&mut internal, &frame).await?;
        }
    }
}

async fn relay_daemon_to_broker_frames<R>(
    state: StubDaemonState,
    mut internal: R,
    external_tx: mpsc::Sender<Frame>,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
{
    loop {
        let Some(frame) = read_relay_frame(&mut internal, &cancel).await? else {
            return Ok(());
        };
        if let Some(frame) = daemon_to_broker_frame(&state, frame).await? {
            if external_tx.send(frame).await.is_err() {
                return Ok(());
            }
        }
    }
}

async fn relay_external_frames<W>(
    mut external: W,
    mut external_rx: mpsc::Receiver<Frame>,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            frame = external_rx.recv() => {
                let Some(frame) = frame else {
                    return Ok(());
                };
                write_frame(&mut external, &frame).await?;
            }
        }
    }
}

async fn read_relay_frame<R>(
    reader: &mut R,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<Frame>>
where
    R: AsyncRead + Unpin,
{
    tokio::select! {
        _ = cancel.cancelled() => Ok(None),
        frame = read_frame(reader) => frame_from_result(frame),
    }
}

fn frame_from_result(result: std::io::Result<Frame>) -> anyhow::Result<Option<Frame>> {
    match result {
        Ok(frame) => Ok(Some(frame)),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn observe_broker_to_daemon_frame(
    state: &StubDaemonState,
    frame: &Frame,
) -> anyhow::Result<ObservedBrokerFrame> {
    let mut observed = ObservedBrokerFrame {
        forward: true,
        error: None,
        transports_to_cancel: Vec::new(),
        cancel_current_transport: false,
        listen_role_transport: false,
        block_open: None,
    };
    {
        let mut control = state.port_forward.control.lock().await;
        match frame.frame_type {
            FrameType::TunnelOpen => {
                control.tunnel_open_count += 1;
                let meta: serde_json::Value = serde_json::from_slice(&frame.meta)?;
                observed.listen_role_transport =
                    meta.get("role").and_then(|role| role.as_str()) == Some("listen");
                if meta.get("role").and_then(|role| role.as_str()) == Some("connect") {
                    if control.connect_tunnel_open_count > 0 {
                        observed.block_open = control
                            .blocked_connect_tunnel_open_after_first
                            .as_mut()
                            .and_then(|state| {
                                let blocked_tx = state.blocked_tx.take();
                                let release_rx = state.release_rx.take();
                                blocked_tx.map(|blocked_tx| (blocked_tx, release_rx))
                            });
                    }
                    control.connect_tunnel_open_count += 1;
                    if control.connect_tunnel_opens_to_drop > 0 {
                        control.connect_tunnel_opens_to_drop -= 1;
                        observed.forward = false;
                        observed.cancel_current_transport = true;
                    }
                }
                if control.close_transports_on_second_session_open && control.tunnel_open_count == 2
                {
                    control.close_transports_on_second_session_open = false;
                    observed.transports_to_cancel = std::mem::take(&mut control.active_transports);
                    control.active_listen_transports.clear();
                    control.active_connect_transports.clear();
                }
            }
            FrameType::TunnelHeartbeat if control.heartbeat_acks_to_drop > 0 => {
                control.heartbeat_acks_to_drop -= 1;
                observed.forward = false;
            }
            FrameType::UdpBind if frame.stream_id >= 3 => {
                if control.udp_connector_bind_errors_remaining > 0 {
                    control.udp_connector_bind_errors_remaining -= 1;
                    observed.forward = false;
                    observed.error = Some(Frame {
                        frame_type: FrameType::Error,
                        flags: 0,
                        stream_id: frame.stream_id,
                        meta: serde_json::to_vec(&serde_json::json!({
                            "code": "port_bind_failed",
                            "message": "forced udp connector bind failure",
                            "fatal": false,
                        }))?,
                        data: Vec::new(),
                    });
                } else {
                    control.active_udp_connector_streams.insert(frame.stream_id);
                    control.opened_udp_connector_streams += 1;
                    control.max_observed_udp_connector_streams = control
                        .max_observed_udp_connector_streams
                        .max(control.active_udp_connector_streams.len());
                }
            }
            FrameType::Close => {
                control
                    .active_udp_connector_streams
                    .remove(&frame.stream_id);
            }
            _ => {}
        }
    }
    if let Some((blocked_tx, release_rx)) = observed.block_open.take() {
        let _ = blocked_tx.send(());
        if let Some(release_rx) = release_rx {
            let _ = release_rx.await;
        }
    }
    Ok(observed)
}

async fn daemon_to_broker_frame(
    state: &StubDaemonState,
    mut frame: Frame,
) -> anyhow::Result<Option<Frame>> {
    let (delay, block_ready, should_forward, resume_timeout_ms, override_tunnel_ready_limits) = {
        let mut control = state.port_forward.control.lock().await;
        if frame.frame_type == FrameType::Close {
            control
                .active_udp_connector_streams
                .remove(&frame.stream_id);
        }
        let (delay, block_ready) = if frame.frame_type == FrameType::TunnelReady {
            let block_ready = if control.session_ready_count > 0 {
                control
                    .blocked_session_ready_after_first
                    .as_mut()
                    .and_then(|state| {
                        let blocked_tx = state.blocked_tx.take();
                        let release_rx = state.release_rx.take();
                        blocked_tx.map(|blocked_tx| (blocked_tx, release_rx))
                    })
            } else {
                None
            };
            let delay = if control.session_ready_count > 0 {
                control.delay_session_ready_after_first
            } else {
                None
            };
            control.session_ready_count += 1;
            (delay, block_ready)
        } else {
            (None, None)
        };
        let resume_timeout_ms = if frame.frame_type == FrameType::TunnelReady {
            control.override_session_ready_resume_timeout_ms
        } else {
            None
        };
        let override_tunnel_ready_limits = if frame.frame_type == FrameType::TunnelReady {
            control.override_tunnel_ready_limits.clone()
        } else {
            None
        };
        let should_forward = !(frame.frame_type == FrameType::TcpConnectOk
            && matches!(
                control.tcp_connect_ok_behavior,
                TcpConnectOkBehavior::DropAll
            ));
        (
            delay,
            block_ready,
            should_forward,
            resume_timeout_ms,
            override_tunnel_ready_limits,
        )
    };
    if let Some((blocked_tx, release_rx)) = block_ready {
        let _ = blocked_tx.send(());
        if let Some(release_rx) = release_rx {
            let _ = release_rx.await;
        }
    }
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    if !should_forward {
        return Ok(None);
    }
    if let Some(resume_timeout_ms) = resume_timeout_ms {
        let mut meta: serde_json::Value = serde_json::from_slice(&frame.meta)?;
        meta["resume_timeout_ms"] = serde_json::json!(resume_timeout_ms);
        frame.meta = serde_json::to_vec(&meta)?;
    }
    if let Some(limits) = override_tunnel_ready_limits {
        let mut meta: serde_json::Value = serde_json::from_slice(&frame.meta)?;
        meta["limits"] = serde_json::to_value(limits)?;
        frame.meta = serde_json::to_vec(&meta)?;
    }
    Ok(Some(frame))
}

fn validate_port_tunnel_upgrade_headers(
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<RpcErrorBody>)> {
    if !header_contains_token(headers, CONNECTION.as_str(), "upgrade") {
        return Err(bad_port_tunnel_request(
            "missing `Connection: Upgrade` header",
        ));
    }
    if !header_eq(headers, UPGRADE.as_str(), UPGRADE_TOKEN) {
        return Err(bad_port_tunnel_request(format!(
            "missing `Upgrade: {UPGRADE_TOKEN}` header"
        )));
    }
    if !header_eq(
        headers,
        TUNNEL_PROTOCOL_VERSION_HEADER,
        TUNNEL_PROTOCOL_VERSION,
    ) {
        return Err(bad_port_tunnel_request(format!(
            "missing `{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}` header"
        )));
    }
    Ok(())
}

fn header_contains_token(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(expected))
}

fn header_eq(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn unsupported_port_tunnel_request() -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody::from_raw_code(
            "unsupported_operation",
            "stub port tunnel support is disabled",
        )),
    )
}

fn bad_port_tunnel_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody::new(RpcErrorCode::BadRequest, message)),
    )
}

fn port_tunnel_unavailable_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(RpcErrorBody::new(
            RpcErrorCode::PortTunnelUnavailable,
            message,
        )),
    )
}
