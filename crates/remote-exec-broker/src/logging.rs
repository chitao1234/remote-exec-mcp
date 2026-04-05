use std::io::IsTerminal;
use std::sync::Once;

use tracing_subscriber::EnvFilter;

const REMOTE_EXEC_LOG_ENV: &str = "REMOTE_EXEC_LOG";
const DEFAULT_FILTER: &str = "warn,remote_exec_broker=info,remote_exec_daemon=info";

pub fn init_logging() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let env_filter = EnvFilter::try_from_env(REMOTE_EXEC_LOG_ENV)
            .or_else(|_| EnvFilter::try_from_default_env())
            .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(true)
            .compact()
            .init();
    });
}

pub fn preview_text(raw: &str, limit: usize) -> String {
    let mut preview = raw.chars().take(limit).collect::<String>();
    if raw.chars().count() > limit {
        preview.push_str("...");
    }
    preview
}
