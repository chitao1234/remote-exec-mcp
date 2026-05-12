use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

pub fn init_compact_stderr_logging(log_env: &str, default_filter: &str) {
    let env_filter = EnvFilter::try_from_env(log_env)
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(true)
        .compact()
        .init();
}

pub fn preview_text(raw: &str, limit: usize) -> String {
    let mut preview = raw.chars().take(limit).collect::<String>();
    if raw.chars().count() > limit {
        preview.push_str("...");
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::preview_text;

    #[test]
    fn preview_text_keeps_short_input() {
        assert_eq!(preview_text("hello", 8), "hello");
    }

    #[test]
    fn preview_text_appends_marker_when_truncated() {
        assert_eq!(preview_text("hello world", 5), "hello...");
    }
}
