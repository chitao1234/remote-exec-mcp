mod format;
mod intercept;

use anyhow::Context;

use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{
    ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecRunningResponse, ExecStartRequest,
    ExecStartResponse, ExecWarning,
};

use crate::mcp_server::ToolCallOutput;
use format::{format_exec_text, format_intercepted_patch_text, prepend_warning_text};
use intercept::maybe_intercept_apply_patch;

struct WriteStdinCompletion {
    output: ToolCallOutput,
    running: bool,
    exit_code: Option<i32>,
}

pub async fn exec_command(
    state: &crate::BrokerState,
    input: ExecCommandInput,
) -> anyhow::Result<ToolCallOutput> {
    crate::request_context::set_current_target(input.target.as_str());
    let cmd_preview = remote_exec_util::preview_text(&input.cmd, 120);
    tracing::info!(
        tool = "exec_command",
        target = %input.target,
        tty = input.tty,
        has_workdir = input.workdir.is_some(),
        has_shell = input.shell.is_some(),
        cmd_preview = %cmd_preview,
        "exec command requested"
    );
    let path_policy = state.exec_path_policy(&input.target).await?;

    if let Some(output) =
        maybe_intercepted_exec_output(state, &input, &input.target, path_policy).await?
    {
        return Ok(output);
    }

    let request = ExecStartRequest {
        cmd: input.cmd.clone(),
        workdir: input.workdir.clone(),
        shell: input.shell.clone(),
        tty: input.tty,
        yield_time_ms: input.yield_time_ms,
        max_output_tokens: input.max_output_tokens,
        login: input.login,
    };
    let response = state.exec_start(&input.target, &request).await?;
    validate_exec_response(&response)?;

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
        target = %input.target,
        intercepted = false,
        running = output.running,
        exit_code = output.exit_code,
        public_session_id = session_id.as_deref().unwrap_or("-"),
        daemon_instance_id = %output.daemon_instance_id,
        "exec command completed"
    );

    exec_command_output(input.target, session_command, response, session_id)
}

pub async fn write_stdin(
    state: &crate::BrokerState,
    input: WriteStdinInput,
) -> anyhow::Result<ToolCallOutput> {
    let session_id = input.session_id.clone();
    let requested_target = input.target.clone();
    if let Some(target) = &requested_target {
        crate::request_context::set_current_target(target.as_str());
    }
    let chars_len = input.chars.as_ref().map_or(0, |chars| chars.len());
    tracing::info!(
        tool = "write_stdin",
        session_id = %session_id,
        requested_target = requested_target.as_deref().unwrap_or("-"),
        chars_len,
        empty_poll = chars_len == 0,
        "exec stdin requested"
    );
    let result = write_stdin_inner(state, input)
        .await
        .context("write_stdin failed")?;
    tracing::info!(
        tool = "write_stdin",
        session_id = %session_id,
        requested_target = requested_target.as_deref().unwrap_or("-"),
        running = result.running,
        exit_code = result.exit_code.unwrap_or(-1),
        "exec stdin completed"
    );
    Ok(result.output)
}

async fn write_stdin_inner(
    state: &crate::BrokerState,
    input: WriteStdinInput,
) -> anyhow::Result<WriteStdinCompletion> {
    let written = state
        .write_exec_session(
            &input.session_id,
            input.target.as_deref(),
            input.chars.unwrap_or_default(),
            input.yield_time_ms,
            input.max_output_tokens,
            input.pty_size,
        )
        .await?;
    validate_exec_response(&written.response)?;

    write_stdin_output(written.record, written.response, written.public_session_id)
}

async fn maybe_intercepted_exec_output(
    state: &crate::BrokerState,
    input: &ExecCommandInput,
    target_name: &str,
    path_policy: PathPolicy,
) -> anyhow::Result<Option<ToolCallOutput>> {
    let Some(intercepted) =
        maybe_intercept_apply_patch(&input.cmd, input.workdir.as_deref(), path_policy)
    else {
        return Ok(None);
    };

    let warnings = vec![ExecWarning::apply_patch_via_exec_command()];
    let output = crate::tools::patch::forward_patch(
        state,
        &input.target,
        intercepted.patch,
        intercepted.workdir,
    )
    .await
    .map_err(|err| anyhow::anyhow!(prepend_warning_text(err.to_string(), &warnings)))?;

    tracing::info!(
        tool = "exec_command",
        target = %target_name,
        intercepted = true,
        "exec command intercepted"
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
            .register_exec_session(
                target,
                response.daemon_session_id.clone(),
                response.response.output().daemon_instance_id.clone(),
                session_command.to_string(),
            )
            .await
            .public_session_id,
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
        format_exec_text(
            Some(session_command.as_str()),
            &response,
            session_id.as_deref(),
        ),
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

fn write_stdin_output(
    record: crate::session_store::SessionRecord,
    response: ExecResponse,
    session_id: Option<String>,
) -> anyhow::Result<WriteStdinCompletion> {
    let output = response.output().clone();
    let text = prepend_warning_text(
        format_exec_text(
            Some(record.session_command.as_str()),
            &response,
            session_id.as_deref(),
        ),
        &output.warnings,
    );
    let result = CommandToolResult {
        target: record.target,
        chunk_id: output.chunk_id,
        wall_time_seconds: output.wall_time_seconds,
        exit_code: output.exit_code,
        session_id,
        session_command: Some(record.session_command),
        original_token_count: output.original_token_count,
        output: output.output,
        warnings: output.warnings,
    };
    Ok(WriteStdinCompletion {
        running: result.session_id.is_some(),
        exit_code: result.exit_code,
        output: ToolCallOutput::text_and_structured(text, serde_json::to_value(result)?),
    })
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

fn validate_exec_response(response: &ExecResponse) -> anyhow::Result<()> {
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
