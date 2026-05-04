use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortListenAcceptRequest, PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest,
    PortListenResponse, PortUdpDatagramReadRequest, PortUdpDatagramReadResponse,
    PortUdpDatagramWriteRequest, RpcErrorBody,
};

pub use remote_exec_host::port_forward::PortForwardState;

pub async fn listen(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortListenRequest>,
) -> Result<Json<PortListenResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::listen_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn listen_accept(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortListenAcceptRequest>,
) -> Result<Json<PortListenAcceptResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::listen_accept_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn listen_close(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortListenCloseRequest>,
) -> Result<Json<EmptyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::listen_close_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn connect(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortConnectRequest>,
) -> Result<Json<PortConnectResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::connect_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn connection_read(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortConnectionReadRequest>,
) -> Result<Json<PortConnectionReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::connection_read_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn connection_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortConnectionWriteRequest>,
) -> Result<Json<EmptyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::connection_write_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn connection_close(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortConnectionCloseRequest>,
) -> Result<Json<EmptyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::connection_close_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn udp_datagram_read(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortUdpDatagramReadRequest>,
) -> Result<Json<PortUdpDatagramReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::udp_datagram_read_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn udp_datagram_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortUdpDatagramWriteRequest>,
) -> Result<Json<EmptyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::udp_datagram_write_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}
