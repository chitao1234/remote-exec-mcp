use anyhow::Context;
use std::time::Instant;

use remote_exec_proto::path::{PathPolicy, linux_path_policy, windows_path_policy};
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWarning, ExecWriteRequest};

use super::exec_format::{
    format_command_text, format_intercepted_patch_text, format_poll_text, prepend_warning_text,
};
use super::exec_intercept::maybe_intercept_apply_patch;
use crate::daemon_client::RpcErrorCode;
use crate::mcp_server::ToolCallOutput;

const APPLY_PATCH_WARNING_CODE: &str = "apply_patch_via_exec_command";
const APPLY_PATCH_WARNING_MESSAGE: &str =
    "Use apply_patch directly rather than through exec_command.";

pub async fn exec_command(
    state: &crate::BrokerState,
    input: ExecCommandInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = Instant::now();
    let target_name = input.target.clone();
    let cmd_preview = crate::logging::preview_text(&input.cmd, 120);
    tracing::info!(
        tool = "exec_command",
        target = %target_name,
        tty = input.tty,
        has_workdir = input.workdir.is_some(),
        has_shell = input.shell.is_some(),
        cmd_preview = %cmd_preview,
        "broker tool started"
    );
    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let path_policy = target_path_policy(target).await?;

    if let Some(output) =
        maybe_intercepted_exec_output(state, &input, &target_name, started, path_policy).await?
    {
        return Ok(output);
    }

    let response = match forward_exec_start(target, &input.target, &input).await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                tool = "exec_command",
                target = %target_name,
                intercepted = false,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "broker tool failed"
            );
            return Err(err);
        }
    };
    validate_exec_response(&response)?;

    let session_command = input.cmd.clone();
    let session_id =
        register_public_session(state, &input.target, &session_command, &response).await;

    tracing::info!(
        tool = "exec_command",
        target = %target_name,
        intercepted = false,
        running = response.running,
        exit_code = response.exit_code,
        public_session_id = session_id.as_deref().unwrap_or("-"),
        daemon_instance_id = %response.daemon_instance_id,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );

    exec_command_output(input.target, session_command, response, session_id)
}

pub async fn write_stdin(
    state: &crate::BrokerState,
    input: WriteStdinInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = Instant::now();
    let session_id = input.session_id.clone();
    let requested_target = input.target.clone();
    let chars_len = input.chars.as_ref().map(|chars| chars.len()).unwrap_or(0);
    tracing::info!(
        tool = "write_stdin",
        session_id = %session_id,
        requested_target = requested_target.as_deref().unwrap_or("-"),
        chars_len,
        empty_poll = chars_len == 0,
        "broker tool started"
    );
    write_stdin_inner(state, input)
        .await
        .inspect(|output| {
            let structured = output
                .structured
                .as_ref()
                .expect("write_stdin tool output should include structured content");
            tracing::info!(
                tool = "write_stdin",
                session_id = %session_id,
                requested_target = requested_target.as_deref().unwrap_or("-"),
                running = structured["session_id"].is_string(),
                exit_code = structured["exit_code"].as_i64().unwrap_or(-1),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "broker tool completed"
            );
        })
        .map_err(|err| {
            tracing::warn!(
                tool = "write_stdin",
                session_id = %session_id,
                requested_target = requested_target.as_deref().unwrap_or("-"),
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "broker tool failed"
            );
            anyhow::anyhow!("write_stdin failed: {err}")
        })
}

async fn write_stdin_inner(
    state: &crate::BrokerState,
    input: WriteStdinInput,
) -> anyhow::Result<ToolCallOutput> {
    let record = state
        .sessions
        .get(&input.session_id)
        .await
        .with_context(|| unknown_process_id_message(&input.session_id))?;

    if let Some(target) = &input.target {
        anyhow::ensure!(
            target == &record.target,
            "session does not belong to target `{target}`"
        );
    }

    let target = state.target(&record.target)?;
    let response = forward_exec_write(state, target, &record, input).await?;
    validate_exec_response(&response)?;

    let session_id = if response.running {
        Some(record.session_id.clone())
    } else {
        state.sessions.remove(&record.session_id).await;
        None
    };

    write_stdin_output(record, response, session_id)
}

async fn maybe_intercepted_exec_output(
    state: &crate::BrokerState,
    input: &ExecCommandInput,
    target_name: &str,
    started: Instant,
    path_policy: PathPolicy,
) -> anyhow::Result<Option<ToolCallOutput>> {
    let Some(intercepted) =
        maybe_intercept_apply_patch(&input.cmd, input.workdir.as_deref(), path_policy)
    else {
        return Ok(None);
    };

    let warnings = vec![apply_patch_warning()];
    let output = crate::tools::patch::forward_patch(
        state,
        &input.target,
        intercepted.patch,
        intercepted.workdir,
    )
    .await
    .map_err(|err| {
        tracing::warn!(
            tool = "exec_command",
            target = %target_name,
            intercepted = true,
            elapsed_ms = started.elapsed().as_millis() as u64,
            error = %err,
            "broker tool failed"
        );
        anyhow::anyhow!(prepend_warning_text(err.to_string(), &warnings))
    })?;

    tracing::info!(
        tool = "exec_command",
        target = %target_name,
        intercepted = true,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );
    Ok(Some(ToolCallOutput::text_and_structured(
        prepend_warning_text(format_intercepted_patch_text(&output), &warnings),
        serde_json::to_value(CommandToolResult {
            target: input.target.clone(),
            chunk_id: None,
            wall_time_seconds: 0.0,
            exit_code: Some(0),
            session_id: None,
            session_command: None,
            original_token_count: None,
            output,
            warnings,
        })?,
    )))
}

fn exec_start_request(input: &ExecCommandInput) -> ExecStartRequest {
    ExecStartRequest {
        cmd: input.cmd.clone(),
        workdir: input.workdir.clone(),
        shell: input.shell.clone(),
        tty: input.tty,
        yield_time_ms: input.yield_time_ms,
        max_output_tokens: input.max_output_tokens,
        login: input.login,
    }
}

async fn forward_exec_start(
    target: &crate::TargetHandle,
    target_name: &str,
    input: &ExecCommandInput,
) -> anyhow::Result<ExecResponse> {
    target
        .exec_start_checked(target_name, &exec_start_request(input))
        .await
}

async fn register_public_session(
    state: &crate::BrokerState,
    target: &str,
    session_command: &str,
    response: &ExecResponse,
) -> Option<String> {
    if !response.running {
        return None;
    }

    let daemon_session_id = response
        .daemon_session_id
        .clone()
        .expect("daemon session id");
    Some(
        state
            .sessions
            .insert(
                target.to_string(),
                daemon_session_id,
                response.daemon_instance_id.clone(),
                session_command.to_string(),
            )
            .await
            .session_id,
    )
}

fn exec_command_output(
    target: String,
    session_command: String,
    response: ExecResponse,
    session_id: Option<String>,
) -> anyhow::Result<ToolCallOutput> {
    let text = prepend_warning_text(
        format_command_text(&session_command, &response, session_id.as_deref()),
        &response.warnings,
    );
    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(CommandToolResult {
            target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            session_command: Some(session_command),
            original_token_count: response.original_token_count,
            output: response.output,
            warnings: response.warnings,
        })?,
    ))
}

async fn forward_exec_write(
    state: &crate::BrokerState,
    target: &crate::TargetHandle,
    record: &crate::session_store::SessionRecord,
    input: WriteStdinInput,
) -> anyhow::Result<ExecResponse> {
    let response = target
        .clear_on_transport_error(
            target
                .exec_write(&ExecWriteRequest {
                    daemon_session_id: record.daemon_session_id.clone(),
                    chars: input.chars.unwrap_or_default(),
                    yield_time_ms: input.yield_time_ms,
                    max_output_tokens: input.max_output_tokens,
                })
                .await,
        )
        .await;

    match response {
        Ok(response) => Ok(response),
        Err(err) if err.is_rpc_error_code(RpcErrorCode::UnknownSession) => {
            state.sessions.remove(&record.session_id).await;
            Err(anyhow::anyhow!(unknown_process_id_message(
                &record.session_id
            )))
        }
        Err(err) => {
            if let Ok(info) = target.target_info().await {
                if info.daemon_instance_id != record.daemon_instance_id {
                    target.clear_cached_daemon_info().await;
                    state.sessions.remove(&record.session_id).await;
                    return Err(anyhow::anyhow!(unknown_process_id_message(
                        &record.session_id
                    )));
                }
            }
            Err(err.into())
        }
    }
}

fn write_stdin_output(
    record: crate::session_store::SessionRecord,
    response: ExecResponse,
    session_id: Option<String>,
) -> anyhow::Result<ToolCallOutput> {
    let text = prepend_warning_text(
        format_poll_text(
            Some(&record.session_command),
            &response,
            session_id.as_deref(),
        ),
        &response.warnings,
    );
    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(CommandToolResult {
            target: record.target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            session_command: Some(record.session_command),
            original_token_count: response.original_token_count,
            output: response.output,
            warnings: response.warnings,
        })?,
    ))
}

fn unknown_process_id_message(session_id: &str) -> String {
    format!("Unknown process id {session_id}")
}

fn validate_exec_response(response: &ExecResponse) -> anyhow::Result<()> {
    if response.running {
        anyhow::ensure!(
            response.exit_code.is_none(),
            "daemon returned malformed exec response: running response unexpectedly included exit_code"
        );
        anyhow::ensure!(
            response
                .daemon_session_id
                .as_deref()
                .is_some_and(|session_id| !session_id.is_empty()),
            "daemon returned malformed exec response: running response missing daemon_session_id"
        );
        return Ok(());
    }

    anyhow::ensure!(
        response.exit_code.is_some(),
        "daemon returned malformed exec response: completed response missing exit_code"
    );
    anyhow::ensure!(
        response.daemon_session_id.is_none(),
        "daemon returned malformed exec response: completed response unexpectedly included daemon_session_id"
    );
    Ok(())
}

async fn target_path_policy(target: &crate::TargetHandle) -> anyhow::Result<PathPolicy> {
    let info = target
        .cached_daemon_info()
        .await
        .context("target info missing after identity verification")?;

    Ok(if info.platform.eq_ignore_ascii_case("windows") {
        windows_path_policy()
    } else {
        linux_path_policy()
    })
}

fn apply_patch_warning() -> ExecWarning {
    ExecWarning {
        code: APPLY_PATCH_WARNING_CODE.to_string(),
        message: APPLY_PATCH_WARNING_MESSAGE.to_string(),
    }
}
