use std::sync::Arc;
use std::time::{Duration, Instant};

use remote_exec_proto::rpc::{
    ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecRunningResponse, ExecWarning,
    RpcErrorCode,
};

use crate::{
    AppState, HostRpcError,
    config::YieldTimeOperation,
    host_path,
    sandbox::{SandboxAccess, SandboxError, authorize_resolved_path},
};

use super::{output, session, timing::EXEC_POLL_INTERVAL};

pub fn resolve_workdir_for_operation(
    state: &Arc<AppState>,
    workdir: Option<&str>,
) -> anyhow::Result<host_path::ResolvedHostPath> {
    Ok(match workdir {
        None => host_path::ResolvedHostPath::new(
            state.config.default_workdir.display().to_string(),
            state.config.default_workdir.clone(),
        ),
        Some(raw) => host_path::resolve_input_path_for_operation(
            &state.config.default_workdir,
            raw,
            state.config.windows_posix_root.as_deref(),
        ),
    })
}

pub fn ensure_resolved_sandbox_access(
    state: &Arc<AppState>,
    access: SandboxAccess,
    path: &host_path::ResolvedHostPath,
) -> Result<(), SandboxError> {
    authorize_resolved_path(state.sandbox.as_ref(), access, path)
}

pub fn internal_error(err: anyhow::Error) -> HostRpcError {
    let message = err.to_string();
    tracing::error!(error = %message, "daemon internal error");
    crate::error::internal(RpcErrorCode::Internal, message)
}

pub(super) async fn poll_until(
    session: &mut session::LiveSession,
    yield_time_ms: u64,
) -> anyhow::Result<String> {
    let deadline = Instant::now() + Duration::from_millis(yield_time_ms);
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = session.read_available().await?;
        if !chunk.is_empty() {
            session.record_output(&chunk);
            output.push_str(&chunk);
        }

        if session.has_exited().await? {
            break;
        }

        tokio::time::sleep(EXEC_POLL_INTERVAL).await;
    }

    Ok(output)
}

pub(super) fn write_yield_time_operation(chars: &str) -> YieldTimeOperation {
    if chars.is_empty() {
        YieldTimeOperation::WriteStdinPoll
    } else {
        YieldTimeOperation::WriteStdinInput
    }
}

pub(super) fn running_response(
    daemon_instance_id: &str,
    daemon_session_id: String,
    session: &session::LiveSession,
    output: String,
    max_output_tokens: Option<u32>,
    warnings: Vec<ExecWarning>,
) -> ExecResponse {
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let snapshot = output::snapshot_output(output, max_output_tokens);
    ExecResponse::Running(ExecRunningResponse {
        daemon_session_id,
        output: ExecOutputResponse {
            daemon_instance_id: daemon_instance_id.to_string(),
            running: true,
            chunk_id: Some(chunk_id()),
            wall_time_seconds,
            exit_code: None,
            original_token_count: Some(snapshot.original_token_count),
            output: snapshot.output,
            warnings,
        },
    })
}

pub(super) fn finish_response(
    daemon_instance_id: &str,
    session: &session::LiveSession,
    output: String,
    max_output_tokens: Option<u32>,
) -> ExecResponse {
    let snapshot = output::snapshot_output(output, max_output_tokens);
    let wall_time_seconds = session.started_at.elapsed().as_secs_f64();
    let exit_code = session.exit_code();
    tracing::info!(exit_code, wall_time_seconds, "built exec response");
    ExecResponse::Completed(ExecCompletedResponse {
        output: ExecOutputResponse {
            daemon_instance_id: daemon_instance_id.to_string(),
            running: false,
            chunk_id: Some(chunk_id()),
            wall_time_seconds,
            exit_code,
            original_token_count: Some(snapshot.original_token_count),
            output: snapshot.output,
            warnings: Vec::new(),
        },
    })
}

// Chunk ids are short-lived response correlation hints, not globally unique
// identifiers. Three random bytes keep them compact while still yielding
// roughly 16 million possibilities.
const CHUNK_ID_RANDOM_BYTES: usize = 3;

fn chunk_id() -> String {
    let bytes = rand::random::<[u8; CHUNK_ID_RANDOM_BYTES]>();
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

#[cfg(test)]
mod tests {
    use super::write_yield_time_operation;
    use crate::config::YieldTimeOperation;

    #[test]
    fn write_yield_time_operation_uses_poll_bucket_for_empty_chars() {
        assert_eq!(
            write_yield_time_operation(""),
            YieldTimeOperation::WriteStdinPoll
        );
    }

    #[test]
    fn write_yield_time_operation_uses_input_bucket_for_non_empty_chars() {
        assert_eq!(
            write_yield_time_operation("hello"),
            YieldTimeOperation::WriteStdinInput
        );
    }
}
