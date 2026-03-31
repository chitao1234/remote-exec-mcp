pub mod session;
pub mod transcript;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rand::RngCore;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest, RpcErrorBody};

use crate::AppState;

pub async fn exec_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let cwd = resolve_workdir(&state, req.workdir.as_deref()).map_err(internal_error)?;
    let argv = shell_argv(req.shell.as_deref(), req.login.unwrap_or(false), &req.cmd);
    let mut session = session::spawn(&argv, &cwd, req.tty).map_err(internal_error)?;

    let deadline = Instant::now()
        + Duration::from_millis(req.yield_time_ms.unwrap_or(10_000).clamp(250, 30_000));
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = poll_once(&mut session).await.map_err(internal_error)?;
        if !chunk.is_empty() {
            output.push_str(&chunk);
            session.transcript.push(chunk.as_bytes());
        }

        if has_exited(&mut session).await.map_err(internal_error)? {
            return Ok(Json(finish_response(None, false, &session, output)));
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let daemon_session_id = uuid::Uuid::new_v4().to_string();
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let original_token_count = output.split_whitespace().count() as u32;
    state
        .sessions
        .lock()
        .await
        .insert(daemon_session_id.clone(), session);

    Ok(Json(ExecResponse {
        daemon_session_id: Some(daemon_session_id),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds,
        exit_code: None,
        original_token_count: Some(original_token_count),
        output,
    }))
}

pub async fn exec_write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let mut sessions = state.sessions.lock().await;
    let session = sessions
        .get_mut(&req.daemon_session_id)
        .ok_or_else(|| rpc_error("unknown_session", "Unknown daemon session"))?;

    if !req.chars.is_empty() && !session.tty {
        return Err(rpc_error(
            "stdin_closed",
            "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open",
        ));
    }

    write_chars(session, &req.chars)
        .await
        .map_err(internal_error)?;
    let output = poll_until(
        session,
        req.chars.is_empty(),
        req.yield_time_ms.unwrap_or(250),
    )
    .await
    .map_err(internal_error)?;
    if has_exited(session).await.map_err(internal_error)? {
        let response = finish_response(None, false, session, output);
        sessions.remove(&req.daemon_session_id);
        return Ok(Json(response));
    }

    Ok(Json(ExecResponse {
        daemon_session_id: Some(req.daemon_session_id),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds: session.started_at.elapsed().as_secs_f64(),
        exit_code: None,
        original_token_count: Some(output.split_whitespace().count() as u32),
        output,
    }))
}

pub fn resolve_workdir(state: &Arc<AppState>, workdir: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(match workdir {
        None => state.config.default_workdir.clone(),
        Some(raw) => {
            let path = PathBuf::from(raw);
            if path.is_absolute() {
                path
            } else {
                state.config.default_workdir.join(path)
            }
        }
    })
}

pub fn rpc_error(
    code: &'static str,
    message: impl Into<String>,
) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: code.to_string(),
            message: message.into(),
        }),
    )
}

pub fn internal_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(RpcErrorBody {
            code: "internal_error".to_string(),
            message: err.to_string(),
        }),
    )
}

fn shell_argv(shell: Option<&str>, login: bool, cmd: &str) -> Vec<String> {
    let shell = shell.unwrap_or("/bin/bash");
    let mode = if login { "-lc" } else { "-c" };
    vec![shell.to_string(), mode.to_string(), cmd.to_string()]
}

fn chunk_id() -> String {
    let mut bytes = [0u8; 3];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

async fn poll_once(session: &mut session::LiveSession) -> anyhow::Result<String> {
    session.read_available().await
}

async fn has_exited(session: &mut session::LiveSession) -> anyhow::Result<bool> {
    session.has_exited().await
}

async fn write_chars(session: &mut session::LiveSession, chars: &str) -> anyhow::Result<()> {
    session.write(chars).await
}

async fn poll_until(
    session: &mut session::LiveSession,
    empty_poll: bool,
    requested_ms: u64,
) -> anyhow::Result<String> {
    let lower = if empty_poll { 5_000 } else { 250 };
    let upper = if empty_poll { 300_000 } else { 30_000 };
    let deadline = Instant::now() + Duration::from_millis(requested_ms.clamp(lower, upper));
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = poll_once(session).await?;
        if !chunk.is_empty() {
            session.transcript.push(chunk.as_bytes());
            output.push_str(&chunk);
        }

        if has_exited(session).await? {
            break;
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Ok(output)
}

fn finish_response(
    daemon_session_id: Option<String>,
    running: bool,
    session: &session::LiveSession,
    output: String,
) -> ExecResponse {
    ExecResponse {
        daemon_session_id,
        running,
        chunk_id: Some(chunk_id()),
        wall_time_seconds: session.started_at.elapsed().as_secs_f64(),
        exit_code: session.exit_code(),
        original_token_count: Some(output.split_whitespace().count() as u32),
        output,
    }
}
