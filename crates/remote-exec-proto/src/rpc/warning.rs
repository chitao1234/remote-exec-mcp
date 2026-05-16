use crate::wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    ApplyPatchViaExecCommand,
    ExecSessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

wire::wire_value_mappings!(WarningCode {
    ApplyPatchViaExecCommand => "apply_patch_via_exec_command",
    ExecSessionLimitApproaching => "exec_session_limit_approaching",
    TransferSkippedUnsupportedEntry => "transfer_skipped_unsupported_entry",
    TransferSkippedSymlink => "transfer_skipped_symlink",
});

#[cfg(test)]
mod tests {
    use super::WarningCode;

    #[test]
    fn warning_code_apply_patch_round_trips() {
        assert_eq!(
            WarningCode::ApplyPatchViaExecCommand.wire_value(),
            "apply_patch_via_exec_command"
        );
        assert_eq!(
            WarningCode::from_wire_value("apply_patch_via_exec_command"),
            Some(WarningCode::ApplyPatchViaExecCommand)
        );
    }
}
