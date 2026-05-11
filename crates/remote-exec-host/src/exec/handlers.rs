use std::sync::Arc;
use std::time::{Duration, Instant};

use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecStartResponse, ExecWarning, ExecWriteRequest, RpcErrorCode,
};
use remote_exec_proto::sandbox::SandboxAccess;

use crate::{AppState, HostRpcError, config::YieldTimeOperation, error::logged_bad_request};

use super::{
    session, shell,
    support::{
        ensure_sandbox_access, finish_response, has_exited, internal_error, poll_once, poll_until,
        resolve_workdir, running_response, write_chars, write_yield_time_operation,
    },
};

const EXEC_START_POLL_INTERVAL_MS: u64 = 25;

pub async fn exec_start_local(
    state: Arc<AppState>,
    req: ExecStartRequest,
) -> Result<ExecResponse, HostRpcError> {
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

        if let Some(response) = completed_response_if_exited(
            state.as_ref(),
            &mut session,
            &mut output,
            req.max_output_tokens,
        )
        .await?
        {
            return Ok(response);
        }

        tokio::time::sleep(Duration::from_millis(EXEC_START_POLL_INTERVAL_MS)).await;
    }

    let started =
        store_running_session(state.as_ref(), session, output, req.max_output_tokens).await?;
    tracing::info!(
        target = %state.config.target,
        daemon_session_id = %started.daemon_session_id,
        warnings = started.response.output().warnings.len(),
        wall_time_seconds = started.response.output().wall_time_seconds,
        output_bytes = started.response.output().output.len(),
        "exec_start left process running"
    );
    Ok(started.response)
}

pub async fn exec_write_local(
    state: Arc<AppState>,
    req: ExecWriteRequest,
) -> Result<ExecResponse, HostRpcError> {
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
        .ok_or_else(|| {
            logged_bad_request(RpcErrorCode::UnknownSession, "Unknown daemon session")
        })?;
    let mut session = session;

    if !req.chars.is_empty() && !session.tty {
        return Err(logged_bad_request(
            RpcErrorCode::StdinClosed,
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
    let mut output = output;
    if let Some(response) = completed_response_if_exited(
        state.as_ref(),
        &mut session,
        &mut output,
        req.max_output_tokens,
    )
    .await?
    {
        session.retire().await;
        tracing::info!(
            target = %state.config.target,
            daemon_session_id = %daemon_session_id,
            exit_code = response.output().exit_code.unwrap_or_default(),
            wall_time_seconds = response.output().wall_time_seconds,
            "exec_write completed session"
        );
        return Ok(response);
    }

    let response = running_session_response(
        state.as_ref(),
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
        wall_time_seconds = response.output().wall_time_seconds,
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
) -> Result<PreparedExecStart, HostRpcError> {
    let cwd = resolve_workdir(state, req.workdir.as_deref()).map_err(internal_error)?;
    ensure_sandbox_access(state, SandboxAccess::ExecCwd, &cwd)
        .map_err(|err| logged_bad_request(RpcErrorCode::SandboxDenied, err.to_string()))?;
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

async fn finish_completed_response(
    state: &AppState,
    session: &mut session::LiveSession,
    mut output: String,
    max_output_tokens: Option<u32>,
) -> Result<ExecResponse, HostRpcError> {
    output.push_str(
        &super::output::drain_after_exit(session)
            .await
            .map_err(internal_error)?,
    );
    Ok(finish_response(
        &state.daemon_instance_id,
        session,
        output,
        max_output_tokens,
    ))
}

async fn completed_response_if_exited(
    state: &AppState,
    session: &mut session::LiveSession,
    output: &mut String,
    max_output_tokens: Option<u32>,
) -> Result<Option<ExecResponse>, HostRpcError> {
    if !has_exited(session).await.map_err(internal_error)? {
        return Ok(None);
    }

    finish_completed_response(state, session, std::mem::take(output), max_output_tokens)
        .await
        .map(Some)
}

async fn store_running_session(
    state: &AppState,
    session: session::LiveSession,
    output: String,
    max_output_tokens: Option<u32>,
) -> Result<ExecStartResponse, HostRpcError> {
    let daemon_session_id = crate::ids::new_exec_session_id().into_string();
    let insert_outcome = state
        .sessions
        .insert(daemon_session_id.clone(), session)
        .await;
    let warnings = session_limit_warnings(state, insert_outcome.crossed_warning_threshold);
    let session = state
        .sessions
        .lock(&daemon_session_id)
        .await
        .ok_or_else(|| internal_error(anyhow::anyhow!("stored daemon session disappeared")))?;
    let response = running_session_response(
        state,
        daemon_session_id.clone(),
        &session,
        output,
        max_output_tokens,
        warnings.clone(),
    );
    Ok(ExecStartResponse {
        daemon_session_id,
        response,
    })
}

fn running_session_response(
    state: &AppState,
    daemon_session_id: String,
    session: &session::LiveSession,
    output: String,
    max_output_tokens: Option<u32>,
    warnings: Vec<ExecWarning>,
) -> ExecResponse {
    running_response(
        &state.daemon_instance_id,
        daemon_session_id,
        session,
        output,
        max_output_tokens,
        warnings,
    )
}

fn session_limit_warnings(state: &AppState, crossed_warning_threshold: bool) -> Vec<ExecWarning> {
    if crossed_warning_threshold {
        vec![ExecWarning::session_limit_approaching(&state.config.target)]
    } else {
        Vec::new()
    }
}

fn ensure_requested_tty_supported(state: &Arc<AppState>, tty: bool) -> Result<(), HostRpcError> {
    if !tty {
        return Ok(());
    }
    if matches!(state.config.pty, crate::config::PtyMode::None) {
        return Err(logged_bad_request(
            RpcErrorCode::TtyDisabled,
            "tty is disabled by daemon config",
        ));
    }
    if !state.supports_pty {
        return Err(logged_bad_request(
            RpcErrorCode::TtyUnsupported,
            "tty is not supported on this host",
        ));
    }
    Ok(())
}

fn resolve_login_request(
    state: &Arc<AppState>,
    requested_login: Option<bool>,
) -> Result<bool, HostRpcError> {
    match requested_login {
        Some(true) if !shell::platform_supports_login_shells() => Err(logged_bad_request(
            RpcErrorCode::LoginShellUnsupported,
            "login shells are not supported on this platform",
        )),
        Some(true) if !state.config.allow_login_shell => Err(logged_bad_request(
            RpcErrorCode::LoginShellDisabled,
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
