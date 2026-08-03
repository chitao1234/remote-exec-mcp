use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

pub fn init_compact_stderr_logging(log_env: &str, default_filter: &str) {
    let env_filter = EnvFilter::try_from_env(log_env)
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(stderr_supports_ansi())
        .with_target(true)
        .compact()
        .init();
}

fn stderr_supports_ansi() -> bool {
    std::io::stderr().is_terminal() && enable_windows_virtual_terminal_processing()
}

#[cfg(not(windows))]
fn enable_windows_virtual_terminal_processing() -> bool {
    true
}

#[cfg(windows)]
fn enable_windows_virtual_terminal_processing() -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, SetConsoleMode,
    };

    let stderr = std::io::stderr();
    let handle = stderr.as_raw_handle();
    let mut mode = 0;

    unsafe {
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }

        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
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
