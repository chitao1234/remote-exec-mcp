#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    ExecSessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

impl WarningCode {
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::ExecSessionLimitApproaching => "exec_session_limit_approaching",
            Self::TransferSkippedUnsupportedEntry => "transfer_skipped_unsupported_entry",
            Self::TransferSkippedSymlink => "transfer_skipped_symlink",
        }
    }
}
