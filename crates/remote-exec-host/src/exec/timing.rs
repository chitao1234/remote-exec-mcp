use std::time::Duration;

pub(super) const EXEC_POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(windows)]
pub(super) const WINDOWS_BACKEND_SMOKE_TIMEOUT: Duration = Duration::from_millis(300);
