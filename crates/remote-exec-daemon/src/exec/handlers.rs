use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWarning, ExecWriteRequest, RpcErrorBody,
};
use remote_exec_proto::sandbox::SandboxAccess;

use crate::{AppState, config::YieldTimeOperation};

use super::{
    session, shell,
    support::{
        ensure_sandbox_access, finish_response, has_exited, internal_error, poll_once, poll_until,
        resolve_workdir, rpc_error, running_response, write_chars, write_yield_time_operation,
    },
};

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
    log_exec_start_request(&state, &req);
    let prepared = prepare_exec_start(&state, &req)?;
    let mut session = session::spawn_with_windows_pty_backend_override(
        &prepared.argv,
        &prepared.cwd,
        req.tty,
        state.windows_pty_backend_override,
        &prepared.process_environment,
    )
    .map_err(internal_error)?;

    let deadline = Instant::now() + Duration::from_millis(prepared.yield_time_ms);
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = poll_once(&mut session).await.map_err(internal_error)?;
        if !chunk.is_empty() {
            output.push_str(&chunk);
            session.record_output(&chunk);
        }

        if has_exited(&mut session).await.map_err(internal_error)? {
            output.push_str(
                &super::output::drain_after_exit(&mut session)
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
    let insert_outcome = state
        .sessions
        .insert(daemon_session_id.clone(), session)
        .await;
    let warnings = if insert_outcome.crossed_warning_threshold {
        vec![ExecWarning::session_limit_approaching(&state.config.target)]
    } else {
        Vec::new()
    };
    let session = state
        .sessions
        .lock(&daemon_session_id)
        .await
        .ok_or_else(|| internal_error(anyhow::anyhow!("stored daemon session disappeared")))?;
    let response = running_response(
        &state.daemon_instance_id,
        daemon_session_id.clone(),
        &session,
        output,
        req.max_output_tokens,
        warnings.clone(),
    );
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %daemon_session_id,
        warnings = warnings.len(),
        wall_time_seconds = response.wall_time_seconds,
        "exec_start left process running"
    );
    Ok(response)
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
    let yield_time_ms = state
        .config
        .yield_time
        .resolve_ms(write_yield_time_operation(&req.chars), req.yield_time_ms);
    let output = poll_until(&mut session, yield_time_ms)
        .await
        .map_err(internal_error)?;
    if has_exited(&mut session).await.map_err(internal_error)? {
        let mut output = output;
        output.push_str(
            &super::output::drain_after_exit(&mut session)
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

    let response = running_response(
        &state.daemon_instance_id,
        daemon_session_id.clone(),
        &session,
        output,
        req.max_output_tokens,
        Vec::new(),
    );
    drop(session);
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %daemon_session_id,
        wall_time_seconds = response.wall_time_seconds,
        "exec_write left process running"
    );
    Ok(response)
}

struct PreparedExecStart {
    cwd: std::path::PathBuf,
    argv: Vec<String>,
    process_environment: crate::config::ProcessEnvironment,
    yield_time_ms: u64,
}

fn prepare_exec_start(
    state: &Arc<AppState>,
    req: &ExecStartRequest,
) -> Result<PreparedExecStart, (StatusCode, Json<RpcErrorBody>)> {
    let cwd = resolve_workdir(state, req.workdir.as_deref()).map_err(internal_error)?;
    ensure_sandbox_access(state, SandboxAccess::ExecCwd, &cwd)
        .map_err(|err| rpc_error("sandbox_denied", err.to_string()))?;
    ensure_requested_tty_supported(state, req.tty)?;
    let login = resolve_login_request(state, req.login)?;
    let shell = shell::selected_shell(
        req.shell.as_deref(),
        &state.default_shell,
        &state.config.process_environment,
        state.config.windows_posix_root.as_deref(),
    )
    .map_err(internal_error)?;
    let mut process_environment = state.config.process_environment.clone();
    shell::apply_session_environment_overrides(
        &mut process_environment,
        &shell,
        state.config.windows_posix_root.as_deref(),
    );
    let yield_time_ms = state
        .config
        .yield_time
        .resolve_ms(YieldTimeOperation::ExecCommand, req.yield_time_ms);
    tracing::debug!(
        target = %state.config.target,
        cwd = %cwd.display(),
        shell = %shell,
        login,
        resolved_yield_time_ms = yield_time_ms,
        "resolved exec request"
    );

    Ok(PreparedExecStart {
        cwd,
        argv: shell::shell_argv(&shell, login, &req.cmd),
        process_environment,
        yield_time_ms,
    })
}

fn ensure_requested_tty_supported(
    state: &Arc<AppState>,
    tty: bool,
) -> Result<(), (StatusCode, Json<RpcErrorBody>)> {
    if !tty {
        return Ok(());
    }
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
    Ok(())
}

fn resolve_login_request(
    state: &Arc<AppState>,
    requested_login: Option<bool>,
) -> Result<bool, (StatusCode, Json<RpcErrorBody>)> {
    match requested_login {
        Some(true) if !shell::platform_supports_login_shells() => Err(rpc_error(
            "login_shell_unsupported",
            "login shells are not supported on this platform",
        )),
        Some(true) if !state.config.allow_login_shell => Err(rpc_error(
            "login_shell_disabled",
            "login shells are disabled by daemon config",
        )),
        Some(login) => Ok(login),
        None if shell::platform_supports_login_shells() => Ok(state.config.allow_login_shell),
        None => Ok(false),
    }
}

fn log_exec_start_request(state: &Arc<AppState>, req: &ExecStartRequest) {
    let cmd_preview = crate::logging::preview_text(&req.cmd, 120);
    tracing::info!(
        target = %state.config.target,
        tty = req.tty,
        has_workdir = req.workdir.is_some(),
        requested_shell = req.shell.as_deref().unwrap_or("<default>"),
        cmd_preview = %cmd_preview,
        "exec_start received"
    );
}
