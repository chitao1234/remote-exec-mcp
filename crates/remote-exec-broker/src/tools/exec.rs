use anyhow::Context;
use std::time::Instant;

use remote_exec_proto::path::{PathPolicy, linux_path_policy, windows_path_policy};
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{
    ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecRunningResponse, ExecStartRequest,
    ExecStartResponse, ExecWarning, ExecWriteRequest, ExecWriteResponse, RpcErrorCode,
};

use super::exec_format::{
    format_command_text, format_intercepted_patch_text, format_poll_text, prepend_warning_text,
};
use super::exec_intercept::maybe_intercept_apply_patch;
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
    crate::request_context::set_current_target(target_name.clone());
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
    validate_exec_start_response(&response)?;

    let session_command = input.cmd.clone();
    let session_id = if response.running() {
        let start_response = exec_start_response(response.clone())?;
        register_public_session(state, &input.target, &session_command, &start_response).await
    } else {
        None
    };
    let output = response.output();

    tracing::info!(
        tool = "exec_command",
        target = %target_name,
        intercepted = false,
        running = output.running,
        exit_code = output.exit_code,
        public_session_id = session_id.as_deref().unwrap_or("-"),
        daemon_instance_id = %output.daemon_instance_id,
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
    if let Some(target) = &requested_target {
        crate::request_context::set_current_target(target.clone());
    }
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
            let loggable = command_tool_result_for_logging(structured);
            tracing::info!(
                tool = "write_stdin",
                session_id = %session_id,
                requested_target = requested_target.as_deref().unwrap_or("-"),
                running = loggable
                    .as_ref()
                    .and_then(|result| result.session_id.as_ref())
                    .is_some(),
                exit_code = loggable
                    .as_ref()
                    .and_then(|result| result.exit_code)
                    .unwrap_or(-1),
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
    crate::request_context::set_current_target(record.target.clone());

    if let Some(target) = &input.target {
        anyhow::ensure!(
            target == &record.target,
            "session does not belong to target `{target}`"
        );
    }

    let target = state.target(&record.target)?;
    let response = forward_exec_write(state, target, &record, input).await?;
    validate_exec_write_response(&response)?;
    let write_response = ExecWriteResponse { response };

    let session_id = if write_response.response.running() {
        Some(record.session_id.clone())
    } else {
        state.sessions.remove(&record.session_id).await;
        None
    };

    write_stdin_output(record, write_response.response, session_id)
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
    response: &ExecStartResponse,
) -> Option<String> {
    if !response.response.running() {
        return None;
    }

    Some(
        state
            .sessions
            .insert(
                target.to_string(),
                response.daemon_session_id.clone(),
                response.response.output().daemon_instance_id.clone(),
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
    let output = response.output().clone();
    let text = prepend_warning_text(
        format_command_text(&session_command, &response, session_id.as_deref()),
        &output.warnings,
    );
    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(CommandToolResult {
            target,
            chunk_id: output.chunk_id,
            wall_time_seconds: output.wall_time_seconds,
            exit_code: output.exit_code,
            session_id,
            session_command: Some(session_command),
            original_token_count: output.original_token_count,
            output: output.output,
            warnings: output.warnings,
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
                    pty_size: input.pty_size,
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
    let output = response.output().clone();
    let text = prepend_warning_text(
        format_poll_text(
            Some(&record.session_command),
            &response,
            session_id.as_deref(),
        ),
        &output.warnings,
    );
    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(CommandToolResult {
            target: record.target,
            chunk_id: output.chunk_id,
            wall_time_seconds: output.wall_time_seconds,
            exit_code: output.exit_code,
            session_id,
            session_command: Some(record.session_command),
            original_token_count: output.original_token_count,
            output: output.output,
            warnings: output.warnings,
        })?,
    ))
}

fn command_tool_result_for_logging(structured: &serde_json::Value) -> Option<CommandToolResult> {
    serde_json::from_value(structured.clone()).ok()
}

fn unknown_process_id_message(session_id: &str) -> String {
    format!("Unknown process id {session_id}")
}

fn exec_start_response(response: ExecResponse) -> anyhow::Result<ExecStartResponse> {
    match response {
        ExecResponse::Running(ExecRunningResponse {
            daemon_session_id,
            output,
        }) => Ok(ExecStartResponse {
            daemon_session_id: daemon_session_id.clone(),
            response: ExecResponse::Running(ExecRunningResponse {
                daemon_session_id,
                output,
            }),
        }),
        ExecResponse::Completed(_) => Err(anyhow::anyhow!(
            "daemon returned malformed exec response: running response missing daemon_session_id"
        )),
    }
}

fn validate_exec_start_response(response: &ExecResponse) -> anyhow::Result<()> {
    match response {
        ExecResponse::Running(ExecRunningResponse { output, .. }) => {
            validate_running_output(output)
        }
        ExecResponse::Completed(ExecCompletedResponse { output }) => {
            validate_completed_output(output)
        }
    }
}

fn validate_exec_write_response(response: &ExecResponse) -> anyhow::Result<()> {
    match response {
        ExecResponse::Running(ExecRunningResponse { output, .. }) => {
            validate_running_output(output)
        }
        ExecResponse::Completed(ExecCompletedResponse { output }) => {
            validate_completed_output(output)
        }
    }
}

fn validate_running_output(response: &ExecOutputResponse) -> anyhow::Result<()> {
    anyhow::ensure!(
        response.exit_code.is_none(),
        "daemon returned malformed exec response: running response unexpectedly included exit_code"
    );
    Ok(())
}

fn validate_completed_output(response: &ExecOutputResponse) -> anyhow::Result<()> {
    anyhow::ensure!(
        response.exit_code.is_some(),
        "daemon returned malformed exec response: completed response missing exit_code"
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

#[cfg(test)]
mod tests {
    use super::command_tool_result_for_logging;

    #[test]
    fn command_tool_result_for_logging_reads_typed_fields() {
        let value = serde_json::json!({
            "target": "local",
            "chunk_id": null,
            "wall_time_seconds": 0.25,
            "exit_code": null,
            "session_id": "session-1",
            "session_command": "sleep 10",
            "original_token_count": null,
            "output": "",
            "warnings": []
        });

        let result = command_tool_result_for_logging(&value).unwrap();
        assert_eq!(result.session_id.as_deref(), Some("session-1"));
        assert_eq!(result.exit_code, None);
    }
}
