use anyhow::Context;
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{ExecStartRequest, ExecWriteRequest};

use super::exec_intercept::maybe_intercept_apply_patch;
use crate::mcp_server::{
    ToolCallError, ToolCallOutput, format_command_text, format_intercepted_patch_text,
    format_poll_text, warning_meta,
};

const APPLY_PATCH_WARNING_CODE: &str = "apply_patch_via_exec_command";
const APPLY_PATCH_WARNING_MESSAGE: &str =
    "Use apply_patch directly rather than through exec_command.";

pub async fn exec_command(
    state: &crate::BrokerState,
    input: ExecCommandInput,
) -> anyhow::Result<ToolCallOutput> {
    if let Some(intercepted) = maybe_intercept_apply_patch(&input.cmd, input.workdir.as_deref()) {
        let output = crate::tools::patch::forward_patch(
            state,
            &input.target,
            intercepted.patch,
            intercepted.workdir,
        )
        .await
        .map_err(|err| {
            anyhow::Error::new(ToolCallError::with_meta(
                err.to_string(),
                Some(warning_meta(
                    APPLY_PATCH_WARNING_CODE,
                    APPLY_PATCH_WARNING_MESSAGE,
                )),
            ))
        })?;

        return Ok(ToolCallOutput::text_structured_meta(
            format_intercepted_patch_text(&output),
            serde_json::to_value(CommandToolResult {
                target: input.target,
                chunk_id: None,
                wall_time_seconds: 0.0,
                exit_code: Some(0),
                session_id: None,
                session_command: None,
                original_token_count: None,
                output,
            })?,
            Some(warning_meta(
                APPLY_PATCH_WARNING_CODE,
                APPLY_PATCH_WARNING_MESSAGE,
            )),
        ));
    }

    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let response = target
        .client
        .exec_start(&ExecStartRequest {
            cmd: input.cmd.clone(),
            workdir: input.workdir.clone(),
            shell: input.shell.clone(),
            tty: input.tty,
            yield_time_ms: input.yield_time_ms,
            max_output_tokens: input.max_output_tokens,
            login: input.login,
        })
        .await?;

    let session_command = input.cmd.clone();
    let session_id = if response.running {
        let daemon_session_id = response
            .daemon_session_id
            .clone()
            .expect("daemon session id");
        Some(
            state
                .sessions
                .insert(
                    input.target.clone(),
                    daemon_session_id,
                    response.daemon_instance_id.clone(),
                    session_command.clone(),
                )
                .await
                .session_id,
        )
    } else {
        None
    };

    Ok(ToolCallOutput::text_and_structured(
        format_command_text(&input.cmd, &response, session_id.as_deref()),
        serde_json::to_value(CommandToolResult {
            target: input.target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            session_command: Some(session_command),
            original_token_count: response.original_token_count,
            output: response.output,
        })?,
    ))
}

pub async fn write_stdin(
    state: &crate::BrokerState,
    input: WriteStdinInput,
) -> anyhow::Result<ToolCallOutput> {
    write_stdin_inner(state, input)
        .await
        .map_err(|err| anyhow::anyhow!("write_stdin failed: {err}"))
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
    let response = match target
        .client
        .exec_write(&ExecWriteRequest {
            daemon_session_id: record.daemon_session_id.clone(),
            chars: input.chars.unwrap_or_default(),
            yield_time_ms: input.yield_time_ms,
            max_output_tokens: input.max_output_tokens,
        })
        .await
    {
        Ok(response) => response,
        Err(err) if err.rpc_code() == Some("unknown_session") => {
            state.sessions.remove(&record.session_id).await;
            return Err(anyhow::anyhow!(unknown_process_id_message(
                &record.session_id
            )));
        }
        Err(err) => {
            if let Ok(info) = target.client.target_info().await
                && info.daemon_instance_id != record.daemon_instance_id
            {
                state.sessions.remove(&record.session_id).await;
                return Err(anyhow::anyhow!(unknown_process_id_message(
                    &record.session_id
                )));
            }
            return Err(err.into());
        }
    };

    let session_id = if response.running {
        Some(record.session_id.clone())
    } else {
        state.sessions.remove(&record.session_id).await;
        None
    };

    Ok(ToolCallOutput::text_and_structured(
        format_poll_text(
            Some(&record.session_command),
            &response,
            session_id.as_deref(),
        ),
        serde_json::to_value(CommandToolResult {
            target: record.target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            session_command: Some(record.session_command),
            original_token_count: response.original_token_count,
            output: response.output,
        })?,
    ))
}

fn unknown_process_id_message(session_id: &str) -> String {
    format!("Unknown process id {session_id}")
}
