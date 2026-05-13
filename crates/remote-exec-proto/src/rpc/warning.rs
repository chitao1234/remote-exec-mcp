use crate::wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    ApplyPatchViaExecCommand,
    ExecSessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

const WARNING_CODE_WIRE_VALUES: &[(WarningCode, &str)] = &[
    (
        WarningCode::ApplyPatchViaExecCommand,
        "apply_patch_via_exec_command",
    ),
    (
        WarningCode::ExecSessionLimitApproaching,
        "exec_session_limit_approaching",
    ),
    (
        WarningCode::TransferSkippedUnsupportedEntry,
        "transfer_skipped_unsupported_entry",
    ),
    (
        WarningCode::TransferSkippedSymlink,
        "transfer_skipped_symlink",
    ),
];

impl WarningCode {
    pub fn wire_value(self) -> &'static str {
        match self {
            Self::ApplyPatchViaExecCommand => "apply_patch_via_exec_command",
            Self::ExecSessionLimitApproaching => "exec_session_limit_approaching",
            Self::TransferSkippedUnsupportedEntry => "transfer_skipped_unsupported_entry",
            Self::TransferSkippedSymlink => "transfer_skipped_symlink",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        wire::from_wire_value(value, WARNING_CODE_WIRE_VALUES)
    }
}

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
