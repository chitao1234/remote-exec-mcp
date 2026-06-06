use remote_exec_proto::public::{EditInput, ReadInput, WriteInput};
use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};

use crate::mcp_server::ToolCallOutput;

pub async fn read(state: &crate::BrokerState, input: ReadInput) -> anyhow::Result<ToolCallOutput> {
    let target_name = input.target.clone();
    crate::request_context::set_current_target(target_name.clone());
    let limit = input
        .limit
        .unwrap_or(state.tools.file.default_read_limit_lines);
    anyhow::ensure!(limit > 0, "read.limit must be greater than zero");
    anyhow::ensure!(
        limit <= state.tools.file.max_read_limit_lines,
        "read.limit {limit} exceeds tools.file.max_read_limit_lines {}",
        state.tools.file.max_read_limit_lines
    );

    tracing::info!(
        tool = "read",
        target = %target_name,
        path = %input.file_path,
        offset = input.offset,
        limit,
        max_bytes = state.tools.file.max_read_bytes,
        "file read requested"
    );
    let response = state
        .file_read(
            &target_name,
            &FileReadRequest {
                path: input.file_path.clone(),
                offset: input.offset,
                limit,
                max_bytes: state.tools.file.max_read_bytes,
            },
        )
        .await?;
    log_read_completed(&target_name, &input.file_path, &response);

    Ok(ToolCallOutput::text(response.output))
}

pub async fn write(
    state: &crate::BrokerState,
    input: WriteInput,
) -> anyhow::Result<ToolCallOutput> {
    let target_name = input.target.clone();
    crate::request_context::set_current_target(target_name.clone());
    tracing::info!(
        tool = "write",
        target = %target_name,
        path = %input.file_path,
        content_len = input.content.len(),
        "file write requested"
    );
    let response = state
        .file_write(
            &target_name,
            &FileWriteRequest {
                path: input.file_path.clone(),
                content: input.content,
                max_bytes: state.tools.file.max_read_bytes,
            },
        )
        .await?;
    log_write_completed(&target_name, &input.file_path, &response);

    let action = if response.created {
        "created"
    } else {
        "updated"
    };
    Ok(ToolCallOutput::text(format!(
        "file {action} successfully with {} lines",
        response.line_count
    )))
}

pub async fn edit(state: &crate::BrokerState, input: EditInput) -> anyhow::Result<ToolCallOutput> {
    let target_name = input.target.clone();
    crate::request_context::set_current_target(target_name.clone());
    tracing::info!(
        tool = "edit",
        target = %target_name,
        path = %input.file_path,
        old_string_len = input.old_string.len(),
        new_string_len = input.new_string.len(),
        replace_all = input.replace_all,
        "file edit requested"
    );
    let response = state
        .file_edit(
            &target_name,
            &FileEditRequest {
                path: input.file_path.clone(),
                old_string: input.old_string,
                new_string: input.new_string,
                replace_all: input.replace_all,
                max_bytes: state.tools.file.max_read_bytes,
            },
        )
        .await?;
    log_edit_completed(&target_name, &input.file_path, &response);

    let replacement_label = if response.replacements == 1 {
        "replacement"
    } else {
        "replacements"
    };
    Ok(ToolCallOutput::text(format!(
        "file updated successfully with {} lines, {} {replacement_label}",
        response.line_count, response.replacements
    )))
}

fn log_read_completed(target: &str, path: &str, response: &FileReadResponse) {
    tracing::info!(
        tool = "read",
        target = %target,
        path = %path,
        lines_returned = response.lines_returned,
        total_lines = response.total_lines,
        eof = response.eof,
        "file read completed"
    );
}

fn log_write_completed(target: &str, path: &str, response: &FileWriteResponse) {
    tracing::info!(
        tool = "write",
        target = %target,
        path = %path,
        created = response.created,
        line_count = response.line_count,
        "file write completed"
    );
}

fn log_edit_completed(target: &str, path: &str, response: &FileEditResponse) {
    tracing::info!(
        tool = "edit",
        target = %target,
        path = %path,
        replacements = response.replacements,
        line_count = response.line_count,
        "file edit completed"
    );
}
