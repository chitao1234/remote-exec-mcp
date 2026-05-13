use crate::wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    ExecSessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

const WARNING_CODE_WIRE_VALUES: &[(WarningCode, &str)] = &[
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
        wire::wire_value(&self, WARNING_CODE_WIRE_VALUES)
    }
}
