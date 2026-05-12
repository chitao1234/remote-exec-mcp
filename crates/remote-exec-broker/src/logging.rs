use std::sync::Once;

const REMOTE_EXEC_LOG_ENV: &str = "REMOTE_EXEC_LOG";
const DEFAULT_FILTER: &str = "warn,remote_exec_broker=info,remote_exec_daemon=info";

pub fn init_logging() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        remote_exec_util::init_compact_stderr_logging(REMOTE_EXEC_LOG_ENV, DEFAULT_FILTER);
    });
}

pub fn preview_text(raw: &str, limit: usize) -> String {
    remote_exec_util::preview_text(raw, limit)
}
