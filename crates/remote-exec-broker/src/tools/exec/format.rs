use remote_exec_proto::rpc::{ExecResponse, ExecWarning};

pub(super) fn format_command_text(
    cmd: &str,
    response: &ExecResponse,
    session_id: Option<&str>,
) -> String {
    format_exec_text(Some(cmd), response, session_id)
}

pub(super) fn format_poll_text(
    cmd: Option<&str>,
    response: &ExecResponse,
    session_id: Option<&str>,
) -> String {
    format_exec_text(cmd, response, session_id)
}

pub(super) fn format_intercepted_patch_text(output: &str) -> String {
    format!("Wall time: 0.0000 seconds\nProcess exited with code 0\nOutput:\n{output}")
}

pub(super) fn prepend_warning_text(text: String, warnings: &[ExecWarning]) -> String {
    if warnings.is_empty() {
        return text;
    }

    let warning_text = if warnings.len() == 1 {
        format!("Warning: {}", warnings[0].message)
    } else {
        format!(
            "Warnings:\n{}",
            warnings
                .iter()
                .map(|warning| format!("- {}", warning.message))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!("{warning_text}\n\n{text}")
}

fn format_exec_text(
    cmd: Option<&str>,
    response: &ExecResponse,
    session_id: Option<&str>,
) -> String {
    let response = response.output();
    let command = cmd
        .map(|cmd| format!("Command: {cmd}\n"))
        .unwrap_or_default();
    let original = response
        .original_token_count
        .map(|count| format!("\nOriginal token count: {count}"))
        .unwrap_or_default();
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };

    format!(
        "{command}Chunk ID: {}\nWall time: {:.3} seconds\n{status}{original}\nOutput:\n{}",
        response
            .chunk_id
            .clone()
            .unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}

#[cfg(test)]
mod tests {
    use super::{
        format_command_text, format_intercepted_patch_text, format_poll_text, prepend_warning_text,
    };
    use remote_exec_proto::rpc::{
        ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecWarning,
    };

    #[test]
    fn format_command_text_includes_original_token_count_when_present() {
        let text = format_command_text("printf hi", &completed_response(), None);

        assert!(text.contains("Original token count: 6"));
    }

    #[test]
    fn format_poll_text_includes_original_token_count_when_present() {
        let text = format_poll_text(None, &completed_response(), None);

        assert!(text.contains("Original token count: 6"));
    }

    #[test]
    fn format_poll_text_includes_command_when_present() {
        let text = format_poll_text(Some("printf hi"), &completed_response(), None);

        assert!(text.starts_with("Command: printf hi\n"));
    }

    #[test]
    fn format_intercepted_patch_text_omits_command_and_chunk_metadata() {
        let text =
            format_intercepted_patch_text("Success. Updated the following files:\nA hello.txt\n");

        assert!(text.contains("Wall time: 0.0000 seconds"));
        assert!(text.contains("Process exited with code 0"));
        assert!(text.contains("Output:\nSuccess. Updated the following files:"));
        assert!(!text.contains("Command:"));
        assert!(!text.contains("Chunk ID:"));
    }

    #[test]
    fn prepend_warning_text_prefixes_single_warning() {
        let text = prepend_warning_text(
            "Process exited with code 0".to_string(),
            &[ExecWarning::from_raw_code("example", "Visible warning")],
        );

        assert_eq!(
            text,
            "Warning: Visible warning\n\nProcess exited with code 0"
        );
    }

    fn completed_response() -> ExecResponse {
        ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id: remote_exec_host::ids::new_instance_id(),
                running: false,
                chunk_id: Some("abc123".to_string()),
                wall_time_seconds: 0.25,
                exit_code: Some(0),
                original_token_count: Some(6),
                output: "one two three".to_string(),
                warnings: Vec::new(),
            },
        })
    }
}
