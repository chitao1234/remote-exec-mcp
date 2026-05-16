use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_tunnel::{EndpointMeta, Frame, FrameType, TcpAcceptMeta};

use super::apply_forward_drop_report;
use super::events::{
    ForwardLoopControl, TunnelFrameOutcome, TunnelRole, classify_transport_failure,
    recoverable_tunnel_frame,
};
use super::generation::StreamIdAllocator;
use super::supervisor::{ForwardRuntime, handle_forward_loop_control};
use super::tunnel::{
    PortTunnel, decode_tunnel_error_frame, decode_tunnel_meta, encode_tunnel_meta,
    format_terminal_tunnel_error, is_backpressure_error, is_recoverable_pressure_tunnel_error,
    is_retryable_transport_error,
};

struct TcpConnectStream {
    listen_stream_id: u32,
    ready: bool,
    listen_eof: bool,
    connect_eof: bool,
    pending_frames: Vec<Frame>,
    pending_bytes: usize,
}

#[derive(Default)]
struct PendingTcpBudget {
    total_bytes: usize,
}

#[derive(Default)]
struct TcpForwardState {
    listen_to_connect: HashMap<u32, u32>,
    connect_streams: HashMap<u32, TcpConnectStream>,
    pending_budget: PendingTcpBudget,
}

pub(super) async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut epoch = runtime.initial_epoch().clone();

    loop {
        let control = run_tcp_forward_epoch(&runtime, &epoch).await?;
        if !handle_forward_loop_control(&runtime, control, &mut epoch, || async {}).await? {
            return Ok(());
        }
    }
}

async fn run_tcp_forward_epoch(
    runtime: &ForwardRuntime,
    epoch: &super::epoch::ForwardEpoch,
) -> anyhow::Result<ForwardLoopControl> {
    let listen_tunnel = epoch.listen_tunnel().clone();
    let connect_tunnel = epoch.connect_tunnel().clone();
    let mut state = TcpForwardState::default();
    let mut connect_stream_ids = StreamIdAllocator::new_odd();

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            frame = listen_tunnel.recv() => {
                if let Some(control) = handle_listen_tunnel_event(
                    runtime,
                    &listen_tunnel,
                    &connect_tunnel,
                    &mut state,
                    &mut connect_stream_ids,
                    frame,
                ).await? {
                    return Ok(control);
                }
            }
            frame = connect_tunnel.recv() => {
                if let Some(control) = handle_connect_tunnel_event(
                    runtime,
                    &listen_tunnel,
                    &connect_tunnel,
                    &mut state,
                    frame,
                ).await? {
                    return Ok(control);
                }
            }
        }
    }
}

async fn handle_listen_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match recoverable_tunnel_frame(
        frame_result,
        "reading tcp listen tunnel",
        "listen-side tcp tunnel error",
        || async { Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Listen)) },
    )
    .await?
    {
        TunnelFrameOutcome::Frame(frame) => frame,
        TunnelFrameOutcome::Control(control) => return Ok(Some(control)),
    };
    match frame.frame_type {
        FrameType::TcpAccept => {
            handle_listen_tcp_accept(
                runtime,
                listen_tunnel,
                connect_tunnel,
                state,
                connect_stream_ids,
                frame,
            )
            .await
        }
        FrameType::TcpData => {
            handle_listen_tcp_data(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::TcpEof => {
            handle_listen_tcp_eof(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::Close => {
            handle_listen_close(runtime, connect_tunnel, state, frame).await;
            Ok(None)
        }
        FrameType::Error => handle_listen_error(runtime, connect_tunnel, state, frame).await,
        FrameType::ForwardDrop => {
            apply_forward_drop_report(&runtime.store, runtime.forward_id().as_str(), &frame).await?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

async fn handle_connect_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match recoverable_tunnel_frame(
        frame_result,
        "reading tcp connect tunnel",
        "connecting tcp forward destination",
        || async {
            close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
            Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect))
        },
    )
    .await?
    {
        TunnelFrameOutcome::Frame(frame) => frame,
        TunnelFrameOutcome::Control(control) => return Ok(Some(control)),
    };
    match frame.frame_type {
        FrameType::TcpConnectOk => {
            handle_connect_tcp_connect_ok(runtime, listen_tunnel, connect_tunnel, state, frame)
                .await
        }
        FrameType::Error => {
            handle_connect_error(runtime, connect_tunnel, listen_tunnel, state, frame).await?;
            Ok(None)
        }
        FrameType::TcpData => handle_connect_tcp_data(listen_tunnel, state, frame).await,
        FrameType::TcpEof => {
            handle_connect_tcp_eof(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::Close => handle_connect_close(runtime, listen_tunnel, state, frame).await,
        _ => Ok(None),
    }
}

async fn handle_listen_tcp_accept(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let accept: TcpAcceptMeta = decode_tunnel_meta(&frame)?;
    if !runtime.try_reserve_active_stream().await {
        let _ = listen_tunnel.close_stream(frame.stream_id).await;
        runtime.record_dropped_stream().await;
        return Ok(None);
    }
    let Some(connect_stream_id) = allocate_connect_stream_id(
        runtime,
        listen_tunnel,
        state,
        connect_stream_ids,
        frame.stream_id,
    )
    .await?
    else {
        return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
    };
    if let Some(control) = send_tcp_connect_open(
        runtime,
        listen_tunnel,
        connect_tunnel,
        frame.stream_id,
        connect_stream_id,
    )
    .await?
    {
        return Ok(Some(control));
    }
    record_tcp_stream_pair(state, frame.stream_id, connect_stream_id);
    tracing::debug!(
        forward_id = %runtime.forward_id(),
        listener_stream_id = accept.listener_stream_id,
        accepted_stream_id = frame.stream_id,
        connect_stream_id,
        "paired tcp tunnel streams"
    );
    Ok(None)
}

async fn allocate_connect_stream_id(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    listen_stream_id: u32,
) -> anyhow::Result<Option<u32>> {
    let Some(connect_stream_id) = connect_stream_ids.next() else {
        debug_assert!(connect_stream_ids.needs_generation_rotation());
        let _ = listen_tunnel.close_stream(listen_stream_id).await;
        settle_active_tcp_stream(runtime, TcpActiveStreamSettlement::Dropped).await;
        close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
        return Ok(None);
    };
    Ok(Some(connect_stream_id))
}

async fn send_tcp_connect_open(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    listen_stream_id: u32,
    connect_stream_id: u32,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    if let Err(err) = connect_tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnect,
            flags: 0,
            stream_id: connect_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: runtime.connect_endpoint().to_string(),
            })?,
            data: Vec::new(),
        })
        .await
    {
        if is_retryable_transport_error(&err) {
            return close_unpaired_listen_stream_after_connect_loss(
                runtime,
                listen_tunnel,
                listen_stream_id,
            )
            .await
            .map(Some);
        }
        settle_active_tcp_stream(runtime, TcpActiveStreamSettlement::Dropped).await;
        return Err(err).context("connecting tcp forward destination");
    }
    Ok(None)
}

fn record_tcp_stream_pair(
    state: &mut TcpForwardState,
    listen_stream_id: u32,
    connect_stream_id: u32,
) {
    state
        .listen_to_connect
        .insert(listen_stream_id, connect_stream_id);
    state.connect_streams.insert(
        connect_stream_id,
        TcpConnectStream {
            listen_stream_id,
            ready: false,
            listen_eof: false,
            connect_eof: false,
            pending_frames: Vec::new(),
            pending_bytes: 0,
        },
    );
}

async fn handle_listen_tcp_data(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(connect_stream_id) = state.listen_to_connect.get(&frame.stream_id).copied() else {
        return Ok(None);
    };
    queue_or_send_tcp_connect_frame(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
        Frame {
            stream_id: connect_stream_id,
            ..frame
        },
    )
    .await
}

async fn handle_listen_tcp_eof(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(connect_stream_id) = state.listen_to_connect.get(&frame.stream_id).copied() else {
        return Ok(None);
    };
    if let Some(stream) = state.connect_streams.get_mut(&connect_stream_id) {
        stream.listen_eof = true;
    }
    if let Some(control) = queue_or_send_tcp_connect_frame(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
        Frame {
            frame_type: frame.frame_type,
            flags: 0,
            stream_id: connect_stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await?
    {
        return Ok(Some(control));
    }
    close_tcp_pair_if_fully_eof(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
    )
    .await
}

async fn handle_listen_close(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) {
    if let Some(connect_stream_id) = state.listen_to_connect.remove(&frame.stream_id) {
        if remove_tcp_stream_entry_and_settle_active(
            runtime,
            state,
            connect_stream_id,
            TcpActiveStreamSettlement::Release,
        )
        .await
        .is_some()
        {
            let _ = connect_tunnel.close_stream(connect_stream_id).await;
        }
    }
}

async fn handle_listen_error(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    if frame.stream_id == runtime.listen_session.listener_stream_id {
        let meta = decode_tunnel_error_frame(&frame);
        if is_recoverable_pressure_tunnel_error(&meta) {
            runtime.record_dropped_stream().await;
            return Ok(None);
        }
        return Err(format_terminal_tunnel_error(&meta)).context("listen-side tcp tunnel error");
    }
    if let Some(connect_stream_id) = state.listen_to_connect.remove(&frame.stream_id) {
        let _ = remove_tcp_stream_entry_and_settle_active(
            runtime,
            state,
            connect_stream_id,
            TcpActiveStreamSettlement::Release,
        )
        .await;
        if let Err(err) = connect_tunnel.close_stream(connect_stream_id).await {
            return classify_transport_failure(
                err,
                "closing tcp connect stream after listen error",
                TunnelRole::Connect,
            )
            .map(Some);
        }
    }
    Ok(None)
}

async fn handle_connect_tcp_connect_ok(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(stream) = state.connect_streams.get_mut(&frame.stream_id) else {
        return Ok(None);
    };
    stream.ready = true;
    let mut pending = Vec::new();
    std::mem::swap(&mut pending, &mut stream.pending_frames);
    match flush_pending_tcp_connect_frames(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        frame.stream_id,
        pending,
    )
    .await?
    {
        TcpFlushResult::Sent { should_remove } => {
            if should_remove {
                state.connect_streams.remove(&frame.stream_id);
            }
            Ok(None)
        }
        TcpFlushResult::Recover(control) => Ok(Some(control)),
    }
}

async fn handle_connect_error(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<()> {
    close_tcp_pair(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        frame.stream_id,
        TcpPairCloseReason::ConnectError,
    )
    .await
}

async fn handle_connect_tcp_data(
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(listen_stream_id) = state
        .connect_streams
        .get(&frame.stream_id)
        .map(|stream| stream.listen_stream_id)
    else {
        return Ok(None);
    };
    if let Err(err) = listen_tunnel
        .send(Frame {
            stream_id: listen_stream_id,
            ..frame
        })
        .await
    {
        return classify_transport_failure(
            err,
            "relaying tcp data to listen tunnel",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    Ok(None)
}

async fn handle_connect_tcp_eof(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(listen_stream_id) = state
        .connect_streams
        .get_mut(&frame.stream_id)
        .map(|stream| {
            stream.connect_eof = true;
            stream.listen_stream_id
        })
    else {
        return Ok(None);
    };
    if let Err(err) = listen_tunnel
        .send(Frame {
            frame_type: frame.frame_type,
            flags: 0,
            stream_id: listen_stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
    {
        return classify_transport_failure(
            err,
            "relaying tcp eof to listen tunnel",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    close_tcp_pair_if_fully_eof(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        frame.stream_id,
    )
    .await
}

async fn handle_connect_close(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    if let Some(stream) = remove_tcp_stream_entry_and_settle_active(
        runtime,
        state,
        frame.stream_id,
        TcpActiveStreamSettlement::Release,
    )
    .await
    {
        if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
            return classify_transport_failure(
                err,
                "closing tcp listen stream",
                TunnelRole::Listen,
            )
            .map(Some);
        }
    }
    Ok(None)
}

async fn close_active_tcp_listen_streams(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
) -> anyhow::Result<()> {
    let streams = std::mem::take(&mut state.connect_streams);
    let dropped_count = streams.len() as u64;
    state.listen_to_connect.clear();
    let mut first_error = None;
    for (_, mut stream) in streams {
        release_pending_budget(&mut state.pending_budget, &mut stream);
        if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
            let classified = classify_transport_failure(
                err,
                "closing tcp listen stream after connect tunnel loss",
                TunnelRole::Listen,
            )
            .map(|_| ());
            if first_error.is_none() {
                first_error = Some(classified);
            }
        }
    }
    runtime
        .record_dropped_streams_and_release_active(dropped_count)
        .await;
    if let Some(result) = first_error {
        result?;
    }
    Ok(())
}

async fn close_unpaired_listen_stream_after_connect_loss(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    listen_stream_id: u32,
) -> anyhow::Result<ForwardLoopControl> {
    let close_result = listen_tunnel.close_stream(listen_stream_id).await;
    settle_active_tcp_stream(runtime, TcpActiveStreamSettlement::Dropped).await;
    if let Err(err) = close_result {
        return classify_transport_failure(
            err,
            "closing tcp listen stream after connect tunnel loss",
            TunnelRole::Listen,
        );
    }
    Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect))
}

async fn queue_or_send_tcp_connect_frame(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(stream_ready) = state
        .connect_streams
        .get(&connect_stream_id)
        .map(|stream| stream.ready)
    else {
        return Ok(None);
    };
    if stream_ready {
        match send_or_classify_connect_tunnel_frame(connect_tunnel, frame).await? {
            ConnectTunnelFrameSend::Sent => {}
            ConnectTunnelFrameSend::Backpressure => {
                close_tcp_pair(
                    runtime,
                    connect_tunnel,
                    listen_tunnel,
                    state,
                    connect_stream_id,
                    TcpPairCloseReason::ConnectPressure,
                )
                .await?;
                return Ok(None);
            }
            ConnectTunnelFrameSend::Retryable => {
                close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
                return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
            }
        }
    } else {
        match try_queue_pending_tcp_connect_frame(runtime, state, connect_stream_id, frame) {
            None | Some(PendingTcpConnectFrameQueueResult::Queued) => {}
            Some(PendingTcpConnectFrameQueueResult::LimitExceeded { listen_stream_id }) => {
                drop_overflowed_pending_tcp_stream(
                    runtime,
                    connect_tunnel,
                    listen_tunnel,
                    state,
                    connect_stream_id,
                    listen_stream_id,
                )
                .await;
                return Ok(None);
            }
        }
    }
    Ok(None)
}

async fn flush_pending_tcp_connect_frames(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    pending_frames: Vec<Frame>,
) -> anyhow::Result<TcpFlushResult> {
    if let Some(stream) = state.connect_streams.get_mut(&connect_stream_id) {
        release_pending_budget(&mut state.pending_budget, stream);
    }
    let mut should_remove = false;
    for frame in pending_frames {
        let is_close = frame.frame_type == FrameType::Close;
        match send_or_classify_connect_tunnel_frame(connect_tunnel, frame).await? {
            ConnectTunnelFrameSend::Sent => {}
            ConnectTunnelFrameSend::Backpressure => {
                close_tcp_pair(
                    runtime,
                    connect_tunnel,
                    listen_tunnel,
                    state,
                    connect_stream_id,
                    TcpPairCloseReason::ConnectPressure,
                )
                .await?;
                return Ok(TcpFlushResult::Sent {
                    should_remove: true,
                });
            }
            ConnectTunnelFrameSend::Retryable => {
                close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
                return Ok(TcpFlushResult::Recover(ForwardLoopControl::RecoverTunnel(
                    TunnelRole::Connect,
                )));
            }
        }
        if is_close {
            if let Some(listen_stream_id) = state
                .connect_streams
                .get(&connect_stream_id)
                .map(|stream| stream.listen_stream_id)
            {
                state.listen_to_connect.remove(&listen_stream_id);
            }
            settle_active_tcp_stream(runtime, TcpActiveStreamSettlement::Release).await;
            should_remove = true;
        }
    }
    Ok(TcpFlushResult::Sent { should_remove })
}

enum TcpFlushResult {
    Sent { should_remove: bool },
    Recover(ForwardLoopControl),
}

enum ConnectTunnelFrameSend {
    Sent,
    Backpressure,
    Retryable,
}

enum TcpPairCloseReason {
    ConnectError,
    ConnectPressure,
}

enum PendingTcpConnectFrameQueueResult {
    Queued,
    LimitExceeded { listen_stream_id: u32 },
}

#[derive(Clone, Copy)]
enum TcpActiveStreamSettlement {
    Release,
    Dropped,
}

async fn send_or_classify_connect_tunnel_frame(
    connect_tunnel: &Arc<PortTunnel>,
    frame: Frame,
) -> anyhow::Result<ConnectTunnelFrameSend> {
    match connect_tunnel
        .send(frame)
        .await
        .context("relaying tcp data to connect tunnel")
    {
        Ok(()) => Ok(ConnectTunnelFrameSend::Sent),
        Err(err) if is_backpressure_error(&err) => Ok(ConnectTunnelFrameSend::Backpressure),
        Err(err) if is_retryable_transport_error(&err) => Ok(ConnectTunnelFrameSend::Retryable),
        Err(err) => Err(err),
    }
}

async fn close_tcp_pair(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    reason: TcpPairCloseReason,
) -> anyhow::Result<()> {
    let settlement = match reason {
        TcpPairCloseReason::ConnectError => TcpActiveStreamSettlement::Release,
        TcpPairCloseReason::ConnectPressure => TcpActiveStreamSettlement::Dropped,
    };
    let Some(stream) =
        remove_tcp_stream_entry_and_settle_active(runtime, state, connect_stream_id, settlement)
            .await
    else {
        return Ok(());
    };
    match reason {
        TcpPairCloseReason::ConnectError => {
            if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
                return classify_transport_failure(
                    err,
                    "closing tcp listen stream after connect error",
                    TunnelRole::Listen,
                )
                .map(|_| ());
            }
            Ok(())
        }
        TcpPairCloseReason::ConnectPressure => {
            let _ = connect_tunnel.close_stream(connect_stream_id).await;
            let _ = listen_tunnel.close_stream(stream.listen_stream_id).await;
            Ok(())
        }
    }
}

async fn close_tcp_pair_if_fully_eof(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(fully_eof) = state
        .connect_streams
        .get(&connect_stream_id)
        .map(|stream| stream.listen_eof && stream.connect_eof)
    else {
        return Ok(None);
    };
    if !fully_eof {
        return Ok(None);
    }
    let stream = remove_tcp_stream_entry_and_settle_active(
        runtime,
        state,
        connect_stream_id,
        TcpActiveStreamSettlement::Release,
    )
    .await
    .expect("fully drained tcp stream exists");
    let listen_stream_id = stream.listen_stream_id;
    if let Err(err) = connect_tunnel.close_stream(connect_stream_id).await {
        if is_retryable_transport_error(&err) {
            if let Err(listen_err) = listen_tunnel.close_stream(listen_stream_id).await {
                classify_transport_failure(
                    listen_err,
                    "closing fully drained tcp listen stream after connect tunnel loss",
                    TunnelRole::Listen,
                )
                .map(Some)
            } else {
                Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)))
            }
        } else {
            Err(err).context("closing fully drained tcp connect stream")
        }
    } else if let Err(err) = listen_tunnel.close_stream(listen_stream_id).await {
        classify_transport_failure(
            err,
            "closing fully drained tcp listen stream",
            TunnelRole::Listen,
        )
        .map(Some)
    } else {
        Ok(None)
    }
}

fn frame_data_bytes(frame: &Frame) -> usize {
    frame.meta.len().saturating_add(frame.data.len())
}

fn try_queue_pending_tcp_connect_frame(
    runtime: &ForwardRuntime,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    frame: Frame,
) -> Option<PendingTcpConnectFrameQueueResult> {
    let stream = state.connect_streams.get_mut(&connect_stream_id)?;
    let added = frame_data_bytes(&frame);
    let next_stream_total = stream.pending_bytes.saturating_add(added);
    let next_forward_total = state.pending_budget.total_bytes.saturating_add(added);
    if next_stream_total > runtime.limits.max_pending_tcp_bytes_per_stream
        || next_forward_total > runtime.limits.max_pending_tcp_bytes_per_forward
    {
        return Some(PendingTcpConnectFrameQueueResult::LimitExceeded {
            listen_stream_id: stream.listen_stream_id,
        });
    }
    stream.pending_bytes = next_stream_total;
    state.pending_budget.total_bytes = next_forward_total;
    stream.pending_frames.push(frame);
    Some(PendingTcpConnectFrameQueueResult::Queued)
}

async fn drop_overflowed_pending_tcp_stream(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    listen_stream_id: u32,
) {
    let _removed = remove_tcp_stream_entry_and_settle_active(
        runtime,
        state,
        connect_stream_id,
        TcpActiveStreamSettlement::Dropped,
    )
    .await
    .expect("pending tcp stream exists");
    let _ = connect_tunnel.close_stream(connect_stream_id).await;
    let _ = listen_tunnel.close_stream(listen_stream_id).await;
}

fn remove_stream_entry(
    state: &mut TcpForwardState,
    connect_stream_id: u32,
) -> Option<TcpConnectStream> {
    let mut stream = state.connect_streams.remove(&connect_stream_id)?;
    release_pending_budget(&mut state.pending_budget, &mut stream);
    state.listen_to_connect.remove(&stream.listen_stream_id);
    Some(stream)
}

async fn remove_tcp_stream_entry_and_settle_active(
    runtime: &ForwardRuntime,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    settlement: TcpActiveStreamSettlement,
) -> Option<TcpConnectStream> {
    let stream = remove_stream_entry(state, connect_stream_id)?;
    settle_active_tcp_stream(runtime, settlement).await;
    Some(stream)
}

fn release_pending_budget(pending_budget: &mut PendingTcpBudget, stream: &mut TcpConnectStream) {
    pending_budget.total_bytes = pending_budget
        .total_bytes
        .saturating_sub(stream.pending_bytes);
    stream.pending_bytes = 0;
}

async fn settle_active_tcp_stream(runtime: &ForwardRuntime, settlement: TcpActiveStreamSettlement) {
    match settlement {
        TcpActiveStreamSettlement::Release => runtime.release_active_stream().await,
        TcpActiveStreamSettlement::Dropped => runtime.record_dropped_active_stream().await,
    }
}

#[cfg(test)]
mod tests;
