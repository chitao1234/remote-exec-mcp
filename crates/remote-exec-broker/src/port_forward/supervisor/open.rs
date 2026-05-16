use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_forward::{ForwardId, ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{EndpointMeta, Frame, FrameType};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    ForwardPortSpec,
};
use tokio_util::sync::CancellationToken;

use super::super::epoch::{ForwardEpoch, INITIAL_FORWARD_GENERATION};
use super::tunnel_open::{
    OpenDataTunnel, OpenListenSession, open_data_tunnel, open_listen_session,
};
use super::{
    ForwardIdentity, ForwardRuntime, LISTEN_SESSION_STREAM_ID, ListenSessionControl,
    ListenSessionParams, OpenedForward,
};
use crate::port_forward::PORT_FORWARD_OPEN_ACK_TIMEOUT;
use crate::port_forward::limits::effective_forward_limits;
use crate::port_forward::side::SideHandle;
use crate::port_forward::store::{PortForwardRecord, PortForwardStore};
use crate::port_forward::tunnel::{
    PortTunnel, decode_tunnel_meta, encode_tunnel_meta, tunnel_error,
};

#[derive(Clone, Copy)]
struct ForwardOpenKind {
    protocol: PublicForwardPortProtocol,
    listen_frame_type: FrameType,
    listen_ok_frame_type: FrameType,
    noun: &'static str,
}

#[derive(Clone, Copy)]
enum ForwardSide {
    Listen,
    Connect,
}

impl ForwardOpenKind {
    fn for_protocol(protocol: PublicForwardPortProtocol) -> Self {
        match protocol {
            PublicForwardPortProtocol::Tcp => Self {
                protocol,
                listen_frame_type: FrameType::TcpListen,
                listen_ok_frame_type: FrameType::TcpListenOk,
                noun: "tcp listener",
            },
            PublicForwardPortProtocol::Udp => Self {
                protocol,
                listen_frame_type: FrameType::UdpBind,
                listen_ok_frame_type: FrameType::UdpBindOk,
                noun: "udp listener",
            },
        }
    }
}

struct ForwardOpenContext {
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    forward_id: ForwardId,
    listen_endpoint: String,
    connect_endpoint: String,
    requested_limits: ForwardPortLimitSummary,
    kind: ForwardOpenKind,
}

struct OpenedTunnels {
    listen: OpenListenSession,
    connect: OpenDataTunnel,
}

struct ReadyListenTunnel {
    tunnel: Arc<PortTunnel>,
    response_endpoint: String,
    session_id: String,
    resume_timeout: std::time::Duration,
    limits: remote_exec_proto::port_tunnel::TunnelLimitSummary,
}

pub async fn open_forward(
    store: PortForwardStore,
    limits: ForwardPortLimitSummary,
    listen_side: SideHandle,
    connect_side: SideHandle,
    spec: &ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_endpoint = normalize_endpoint(&spec.listen_endpoint)?;
    let connect_endpoint = ensure_nonzero_connect_endpoint(&spec.connect_endpoint)?;
    open_protocol_forward(
        listen_side,
        connect_side,
        store,
        listen_endpoint,
        connect_endpoint,
        limits,
        ForwardOpenKind::for_protocol(spec.protocol),
    )
    .await
}

async fn open_protocol_forward(
    listen_side: SideHandle,
    connect_side: SideHandle,
    store: PortForwardStore,
    listen_endpoint: String,
    connect_endpoint: String,
    limits: ForwardPortLimitSummary,
    kind: ForwardOpenKind,
) -> anyhow::Result<OpenedForward> {
    let forward_id = remote_exec_host::ids::new_forward_id();
    let initial_generation = INITIAL_FORWARD_GENERATION;
    let opened_listen = open_listen_session_for_forward(
        &listen_side,
        forward_id.as_str(),
        kind,
        initial_generation,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    let opened_connect = open_connect_tunnel_for_forward(
        &connect_side,
        forward_id.as_str(),
        kind,
        initial_generation,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    build_opened_forward(
        ForwardOpenContext {
            store,
            listen_side,
            connect_side,
            forward_id,
            listen_endpoint,
            connect_endpoint,
            requested_limits: limits,
            kind,
        },
        OpenedTunnels {
            listen: opened_listen,
            connect: opened_connect,
        },
    )
    .await
}

async fn open_listen_session_for_forward(
    listen_side: &SideHandle,
    forward_id: &str,
    kind: ForwardOpenKind,
    generation: u64,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenListenSession> {
    open_listen_session(
        listen_side,
        forward_id,
        kind.protocol,
        generation,
        None,
        max_queued_bytes,
    )
    .await
}

async fn open_connect_tunnel_for_forward(
    connect_side: &SideHandle,
    forward_id: &str,
    kind: ForwardOpenKind,
    generation: u64,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenDataTunnel> {
    open_data_tunnel(
        connect_side,
        forward_id,
        kind.protocol,
        generation,
        max_queued_bytes,
    )
    .await
    .with_context(|| {
        open_context(
            kind,
            ForwardSide::Connect,
            connect_side.name(),
            "data tunnel",
        )
    })
}

async fn build_opened_forward(
    context: ForwardOpenContext,
    opened: OpenedTunnels,
) -> anyhow::Result<OpenedForward> {
    let ForwardOpenContext {
        store,
        listen_side,
        connect_side,
        forward_id,
        listen_endpoint,
        connect_endpoint,
        requested_limits,
        kind,
    } = context;
    let connect_tunnel = opened.connect.tunnel;
    let ready_listen = open_listener_for_forward(
        &listen_side,
        &listen_endpoint,
        kind,
        opened.listen,
        &connect_tunnel,
    )
    .await?;
    let ReadyListenTunnel {
        tunnel: listen_tunnel,
        response_endpoint: listen_response,
        session_id,
        resume_timeout,
        limits: listen_limits,
    } = ready_listen;
    let limits = effective_forward_limits(requested_limits, &listen_limits, &opened.connect.limits);
    let initial_generation = INITIAL_FORWARD_GENERATION;
    let listener_stream_id = LISTEN_SESSION_STREAM_ID;
    let listen_session = Arc::new(ListenSessionControl::new(ListenSessionParams {
        side: listen_side.clone(),
        forward_id: forward_id.clone(),
        session_id,
        protocol: kind.protocol,
        listener_stream_id,
        resume_timeout,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        generation: initial_generation,
        tunnel: listen_tunnel,
    }));
    let initial_epoch = ForwardEpoch::new(
        initial_generation,
        listen_session
            .current_tunnel()
            .await
            .expect("listen tunnel"),
        connect_tunnel,
    );

    let cancel = CancellationToken::new();
    let identity = ForwardIdentity::new(
        forward_id.clone(),
        listen_side.clone(),
        connect_side.clone(),
        kind.protocol,
        connect_endpoint.clone(),
    );
    let runtime = ForwardRuntime::new(
        identity,
        limits.into(),
        store,
        listen_session.clone(),
        initial_epoch,
        cancel.clone(),
    );
    Ok(OpenedForward {
        record: PortForwardRecord::new(
            ForwardPortEntry::new_open(
                forward_id,
                listen_side.name().to_string(),
                listen_response,
                connect_side.name().to_string(),
                connect_endpoint,
                kind.protocol,
                limits,
            ),
            listen_session,
            cancel,
        ),
        runtime,
    })
}

async fn open_listener_for_forward(
    listen_side: &SideHandle,
    listen_endpoint: &str,
    kind: ForwardOpenKind,
    opened: OpenListenSession,
    connect_tunnel: &Arc<PortTunnel>,
) -> anyhow::Result<ReadyListenTunnel> {
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
        limits,
    } = opened;
    let listener_stream_id = LISTEN_SESSION_STREAM_ID;
    let listener_open_context = open_context(
        kind,
        ForwardSide::Listen,
        listen_side.name(),
        listen_endpoint,
    );
    listen_tunnel
        .send(Frame {
            frame_type: kind.listen_frame_type,
            flags: 0,
            stream_id: listener_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: listen_endpoint.to_string(),
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| listener_open_context.clone())?;
    let response_endpoint = match wait_for_listener_ready(
        &listen_tunnel,
        listener_stream_id,
        kind.listen_ok_frame_type,
        listener_open_context,
        open_context(
            kind,
            ForwardSide::Listen,
            listen_side.name(),
            listen_endpoint,
        ),
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(err) => {
            connect_tunnel.abort().await;
            return Err(err);
        }
    };
    Ok(ReadyListenTunnel {
        tunnel: listen_tunnel,
        response_endpoint,
        session_id,
        resume_timeout,
        limits,
    })
}

fn open_context(kind: ForwardOpenKind, side: ForwardSide, target: &str, endpoint: &str) -> String {
    match side {
        ForwardSide::Listen => format!("opening {} on `{target}` at `{endpoint}`", kind.noun),
        ForwardSide::Connect => format!(
            "opening {} data port tunnel on `{target}`",
            forward_protocol_name(kind.protocol)
        ),
    }
}

fn forward_protocol_name(protocol: PublicForwardPortProtocol) -> &'static str {
    match protocol {
        PublicForwardPortProtocol::Tcp => "tcp",
        PublicForwardPortProtocol::Udp => "udp",
    }
}

async fn wait_for_listener_ready(
    tunnel: &Arc<PortTunnel>,
    stream_id: u32,
    ok_type: FrameType,
    open_context: String,
    wait_context: String,
) -> anyhow::Result<String> {
    loop {
        let frame = tokio::time::timeout(PORT_FORWARD_OPEN_ACK_TIMEOUT, tunnel.recv())
            .await
            .map_err(|_| {
                anyhow::anyhow!("timed out waiting for port forward listener acknowledgement")
            })?
            .with_context(|| wait_context.clone())?;
        match frame.frame_type {
            frame_type if frame_type == ok_type && frame.stream_id == stream_id => {
                return Ok(decode_tunnel_meta::<EndpointMeta>(&frame)?.endpoint);
            }
            FrameType::Error if frame.stream_id == stream_id => {
                return Err(tunnel_error(&frame)).with_context(|| open_context.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED, TunnelErrorMeta, TunnelLimitSummary,
    };

    use super::*;
    use crate::port_forward::BrokerPortForwardLimits;
    use crate::port_forward::test_support::ScriptedTunnelIo;

    #[tokio::test]
    async fn listen_open_failure_aborts_already_opened_connect_tunnel() {
        let listen_io = ScriptedTunnelIo::default();
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_io.clone()).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
        let limits = TunnelLimitSummary {
            max_active_tcp_streams: 32,
            max_udp_peers: 32,
            max_queued_bytes: remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES,
        };
        listen_io.push_read_frame(&Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: LISTEN_SESSION_STREAM_ID,
            meta: encode_tunnel_meta(&TunnelErrorMeta::new(
                TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED,
                "listen refused",
                false,
                None,
            ))
            .unwrap(),
            data: Vec::new(),
        });

        let result = build_opened_forward(
            ForwardOpenContext {
                store: PortForwardStore::default(),
                listen_side: SideHandle::local().unwrap(),
                connect_side: SideHandle::local().unwrap(),
                forward_id: ForwardId::new("fwd_test"),
                listen_endpoint: "127.0.0.1:10000".to_string(),
                connect_endpoint: "127.0.0.1:10001".to_string(),
                requested_limits: BrokerPortForwardLimits::default().public_summary(),
                kind: ForwardOpenKind::for_protocol(PublicForwardPortProtocol::Tcp),
            },
            OpenedTunnels {
                listen: OpenListenSession {
                    tunnel: listen_tunnel,
                    session_id: "session_test".to_string(),
                    resume_timeout: Duration::from_secs(5),
                    limits: limits.clone(),
                },
                connect: OpenDataTunnel {
                    tunnel: connect_tunnel.clone(),
                    limits,
                },
            },
        )
        .await;

        let err = match result {
            Ok(_) => panic!("listen open failure should abort forward construction"),
            Err(err) => err,
        };
        assert!(
            format!("{err:#}").contains(&format!(
                "{TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED}: listen refused"
            )),
            "unexpected error: {err:#}"
        );
        tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                let result = connect_tunnel
                    .send(Frame {
                        frame_type: FrameType::Close,
                        flags: 0,
                        stream_id: 99,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                if result.is_err() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("listen-open failure should abort the connect tunnel immediately");
    }
}
