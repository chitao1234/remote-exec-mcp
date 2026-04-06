mod locale;
mod output;
pub mod session;
pub(crate) mod shell;
pub mod store;
pub mod transcript;
#[cfg(windows)]
mod winpty;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rand::RngCore;
use remote_exec_proto::path::{
    PathPolicy, is_absolute_for_policy, linux_path_policy, normalize_for_system,
    windows_path_policy,
};
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWarning, ExecWriteRequest, RpcErrorBody,
};
use remote_exec_proto::sandbox::{SandboxAccess, SandboxError, authorize_path};

use crate::AppState;

pub async fn exec_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    exec_start_local(state, req).await.map(Json)
}

pub async fn exec_start_local(
    state: Arc<AppState>,
    req: ExecStartRequest,
) -> Result<ExecResponse, (StatusCode, Json<RpcErrorBody>)> {
    let cmd_preview = crate::logging::preview_text(&req.cmd, 120);
    tracing::info!(
        target = %state.config.target,
        tty = req.tty,
        has_workdir = req.workdir.is_some(),
        requested_shell = req.shell.as_deref().unwrap_or("<default>"),
        cmd_preview = %cmd_preview,
        "exec_start received"
    );
    let cwd = resolve_workdir(&state, req.workdir.as_deref()).map_err(internal_error)?;
    ensure_sandbox_access(&state, SandboxAccess::ExecCwd, &cwd)
        .map_err(|err| rpc_error("sandbox_denied", err.to_string()))?;
    if req.tty {
        if matches!(state.config.pty, crate::config::PtyMode::None) {
            return Err(rpc_error(
                "tty_disabled",
                "tty is disabled by daemon config",
            ));
        }
        if !state.supports_pty {
            return Err(rpc_error(
                "tty_unsupported",
                "tty is not supported on this host",
            ));
        }
    }
    let login = match req.login {
        Some(true) if !shell::platform_supports_login_shells() => {
            return Err(rpc_error(
                "login_shell_unsupported",
                "login shells are not supported on this platform",
            ));
        }
        Some(true) if !state.config.allow_login_shell => {
            return Err(rpc_error(
                "login_shell_disabled",
                "login shells are disabled by daemon config",
            ));
        }
        Some(login) => login,
        None if shell::platform_supports_login_shells() => state.config.allow_login_shell,
        None => false,
    };
    let shell = shell::selected_shell(
        req.shell.as_deref(),
        &state.default_shell,
        &state.config.process_environment,
    )
    .map_err(internal_error)?;
    tracing::debug!(
        target = %state.config.target,
        cwd = %cwd.display(),
        shell = %shell,
        login,
        "resolved exec request"
    );
    let argv = shell::shell_argv(&shell, login, &req.cmd);
    let mut session = session::spawn_with_windows_pty_backend_override(
        &argv,
        &cwd,
        req.tty,
        state.windows_pty_backend_override,
        &state.config.process_environment,
    )
    .map_err(internal_error)?;

    let deadline = Instant::now()
        + Duration::from_millis(req.yield_time_ms.unwrap_or(10_000).clamp(250, 30_000));
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = poll_once(&mut session).await.map_err(internal_error)?;
        if !chunk.is_empty() {
            output.push_str(&chunk);
            session.record_output(&chunk);
        }

        if has_exited(&mut session).await.map_err(internal_error)? {
            output.push_str(
                &output::drain_after_exit(&mut session)
                    .await
                    .map_err(internal_error)?,
            );
            return Ok(finish_response(
                &state.daemon_instance_id,
                None,
                false,
                &session,
                output,
                req.max_output_tokens,
            ));
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let daemon_session_id = uuid::Uuid::new_v4().to_string();
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let snapshot = output::snapshot_output(output, req.max_output_tokens);
    let insert_outcome = state
        .sessions
        .insert(daemon_session_id.clone(), session)
        .await;
    let warnings = if insert_outcome.crossed_warning_threshold {
        vec![ExecWarning::session_limit_approaching(&state.config.target)]
    } else {
        Vec::new()
    };
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %daemon_session_id,
        warnings = warnings.len(),
        wall_time_seconds,
        "exec_start left process running"
    );

    Ok(ExecResponse {
        daemon_session_id: Some(daemon_session_id),
        daemon_instance_id: state.daemon_instance_id.clone(),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds,
        exit_code: None,
        original_token_count: Some(snapshot.original_token_count),
        output: snapshot.output,
        warnings,
    })
}

pub async fn exec_write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    exec_write_local(state, req).await.map(Json)
}

pub async fn exec_write_local(
    state: Arc<AppState>,
    req: ExecWriteRequest,
) -> Result<ExecResponse, (StatusCode, Json<RpcErrorBody>)> {
    let daemon_session_id = req.daemon_session_id;
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %daemon_session_id,
        chars_len = req.chars.len(),
        empty_poll = req.chars.is_empty(),
        "exec_write received"
    );
    let session = state
        .sessions
        .lock(&daemon_session_id)
        .await
        .ok_or_else(|| rpc_error("unknown_session", "Unknown daemon session"))?;
    let mut session = session;

    if !req.chars.is_empty() && !session.tty {
        return Err(rpc_error(
            "stdin_closed",
            "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open",
        ));
    }

    write_chars(&mut session, &req.chars)
        .await
        .map_err(internal_error)?;
    let output = poll_until(
        &mut session,
        req.chars.is_empty(),
        req.yield_time_ms.unwrap_or(250),
    )
    .await
    .map_err(internal_error)?;
    if has_exited(&mut session).await.map_err(internal_error)? {
        let mut output = output;
        output.push_str(
            &output::drain_after_exit(&mut session)
                .await
                .map_err(internal_error)?,
        );
        let response = finish_response(
            &state.daemon_instance_id,
            None,
            false,
            &session,
            output,
            req.max_output_tokens,
        );
        session.retire().await;
        tracing::info!(
            target = %state.config.target,
            daemon_session_id = %daemon_session_id,
            exit_code = response.exit_code.unwrap_or_default(),
            wall_time_seconds = response.wall_time_seconds,
            "exec_write completed session"
        );
        return Ok(response);
    }
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let snapshot = output::snapshot_output(output, req.max_output_tokens);
    drop(session);
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %daemon_session_id,
        wall_time_seconds,
        "exec_write left process running"
    );

    Ok(ExecResponse {
        daemon_session_id: Some(daemon_session_id),
        daemon_instance_id: state.daemon_instance_id.clone(),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds,
        exit_code: None,
        original_token_count: Some(snapshot.original_token_count),
        output: snapshot.output,
        warnings: Vec::new(),
    })
}

pub fn resolve_workdir(state: &Arc<AppState>, workdir: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(match workdir {
        None => state.config.default_workdir.clone(),
        Some(raw) => {
            if is_absolute_for_policy(host_path_policy(), raw) {
                PathBuf::from(normalize_for_system(host_path_policy(), raw))
            } else {
                state
                    .config
                    .default_workdir
                    .join(normalize_for_system(host_path_policy(), raw))
            }
        }
    })
}

pub fn resolve_input_path(base: &Path, raw: &str) -> PathBuf {
    if is_absolute_for_policy(host_path_policy(), raw) {
        PathBuf::from(normalize_for_system(host_path_policy(), raw))
    } else {
        base.join(normalize_for_system(host_path_policy(), raw))
    }
}

fn host_path_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

pub fn ensure_sandbox_access(
    state: &Arc<AppState>,
    access: SandboxAccess,
    path: &Path,
) -> Result<(), SandboxError> {
    authorize_path(host_path_policy(), state.sandbox.as_ref(), access, path)
}

pub fn rpc_error(
    code: &'static str,
    message: impl Into<String>,
) -> (StatusCode, Json<RpcErrorBody>) {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: code.to_string(),
            message,
        }),
    )
}

pub fn internal_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    let message = err.to_string();
    tracing::error!(error = %message, "daemon internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(RpcErrorBody {
            code: "internal_error".to_string(),
            message,
        }),
    )
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
            session.record_output(&chunk);
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
    daemon_instance_id: &str,
    daemon_session_id: Option<String>,
    running: bool,
    session: &session::LiveSession,
    output: String,
    max_output_tokens: Option<u32>,
) -> ExecResponse {
    let snapshot = output::snapshot_output(output, max_output_tokens);
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let exit_code = session.exit_code();
    tracing::info!(
        daemon_session_id = daemon_session_id.as_deref().unwrap_or("-"),
        running,
        exit_code,
        wall_time_seconds,
        "built exec response"
    );
    ExecResponse {
        daemon_session_id,
        daemon_instance_id: daemon_instance_id.to_string(),
        running,
        chunk_id: Some(chunk_id()),
        wall_time_seconds,
        exit_code,
        original_token_count: Some(snapshot.original_token_count),
        output: snapshot.output,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::shell;

    #[cfg(unix)]
    #[test]
    fn shell_argv_uses_dash_c_for_non_login_shells() {
        assert_eq!(
            shell::shell_argv("/bin/sh", false, "printf ok"),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_argv_uses_dash_l_then_dash_c_for_login_shells() {
        assert_eq!(
            shell::shell_argv("/bin/sh", true, "printf ok"),
            vec![
                "/bin/sh".to_string(),
                "-l".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_argv_uses_cmd_c_for_cmd_shells() {
        assert_eq!(
            shell::shell_argv("cmd.exe", false, "echo ok"),
            vec![
                "cmd.exe".to_string(),
                "/D".to_string(),
                "/C".to_string(),
                "echo ok".to_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_argv_uses_command_for_powershell_family() {
        assert_eq!(
            shell::shell_argv("pwsh", false, "Write-Output ok"),
            vec![
                "pwsh".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Write-Output ok".to_string(),
            ]
        );
    }
}
