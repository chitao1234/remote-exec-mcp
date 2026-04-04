#[path = "../support/mod.rs"]
mod support;

use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::sync::OnceLock;

use remote_exec_daemon::config::ProcessEnvironment;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest};
#[cfg(windows)]
use support::spawn::WindowsPtyTestBackend;

#[cfg(unix)]
const TEST_SHELL: &str = "/bin/sh";
#[cfg(windows)]
const TEST_SHELL: &str = "powershell.exe";
#[cfg(windows)]
const WINDOWS_CMD_SHELL: &str = "cmd.exe";
#[cfg(windows)]
const WINDOWS_ENV_OVERLAY_OUTPUT: &str = "dumb|1|cat|cat|1|||";
// Commands in these tests are expected to finish in a single RPC response, but the daemon only
// guarantees a minimum 250 ms wait. Use a wider window so full-suite load does not turn them into
// legitimate live-session responses.
const COMPLETED_COMMAND_YIELD_MS: u64 = 5_000;

#[cfg(windows)]
async fn lock_windows_pty_test_matrix() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    // Winpty-backed integration tests can interfere with each other when the default test
    // harness runs them concurrently. Serialize only the PTY backend matrix helpers.
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[cfg(windows)]
macro_rules! for_each_windows_pty_backend {
    ($backend:ident, $fixture:ident, $body:block) => {{
        let _guard = lock_windows_pty_test_matrix().await;
        for $backend in support::spawn::supported_windows_pty_backends() {
            let $fixture =
                support::spawn::spawn_daemon_for_windows_pty_backend("builder-a", $backend).await;
            $body
        }
    }};
}

#[cfg(windows)]
macro_rules! for_each_windows_pty_backend_with_environment {
    ($backend:ident, $fixture:ident, $environment:expr, $body:block) => {{
        let _guard = lock_windows_pty_test_matrix().await;
        for $backend in support::spawn::supported_windows_pty_backends() {
            let $fixture =
                support::spawn::spawn_daemon_for_windows_pty_backend_with_process_environment(
                    "builder-a",
                    $backend,
                    $environment.clone(),
                )
                .await;
            $body
        }
    }};
}

#[cfg(windows)]
fn windows_start_request(
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
) -> ExecStartRequest {
    ExecStartRequest {
        cmd: cmd.to_string(),
        workdir: None,
        shell: Some(TEST_SHELL.to_string()),
        tty,
        yield_time_ms,
        max_output_tokens,
        login: Some(false),
    }
}

#[cfg(windows)]
fn windows_cmd_start_request(
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
) -> ExecStartRequest {
    ExecStartRequest {
        cmd: cmd.to_string(),
        workdir: None,
        shell: Some(WINDOWS_CMD_SHELL.to_string()),
        tty,
        yield_time_ms,
        max_output_tokens,
        login: Some(false),
    }
}

#[cfg(windows)]
fn windows_probe_command(mode: &str) -> String {
    let probe = Path::new(env!("CARGO_BIN_EXE_pty_input_probe"))
        .display()
        .to_string();
    format!("& '{probe}' {mode}")
}

#[cfg(windows)]
fn strip_terminal_noise(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                    continue;
                }
                Some(']') => {
                    chars.next();
                    let mut prev = None;
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (prev == Some('\u{1b}') && next == '\\') {
                            break;
                        }
                        prev = Some(next);
                    }
                    continue;
                }
                _ => {}
            }
        }

        if ch != '\u{7}' {
            out.push(ch);
        }
    }

    out
}

#[cfg(windows)]
async fn assert_windows_tty_session_answers_terminal_queries(
    fixture: &support::fixture::DaemonFixture,
    backend: WindowsPtyTestBackend,
) {
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo hello & ping -n 30 127.0.0.1 >nul".to_string(),
                workdir: None,
                shell: Some("cmd.exe".to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    assert!(
        started.running,
        "{} start response: {started:#?}",
        backend.name()
    );
    let session_id = started
        .daemon_session_id
        .clone()
        .expect("tty start should create a live session");

    let polled = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id,
                chars: String::new(),
                yield_time_ms: Some(5_000),
                max_output_tokens: Some(2_000),
            },
        )
        .await;

    assert!(
        polled.running,
        "{} poll response: {polled:#?}",
        backend.name()
    );

    let combined_output =
        strip_terminal_noise(&format!("{}{}", started.output, polled.output)).to_ascii_lowercase();
    assert!(
        combined_output.contains("hello"),
        "{} combined output did not contain hello: {combined_output:?}",
        backend.name()
    );
    assert!(
        !combined_output.contains("\u{1b}[5n"),
        "{} combined output leaked DSR probe: {combined_output:?}",
        backend.name()
    );
    assert!(
        !combined_output.contains("\u{1b}[6n"),
        "{} combined output leaked CPR probe: {combined_output:?}",
        backend.name()
    );
}

#[cfg(windows)]
async fn assert_windows_bare_lf_advances_pty_line_reader(
    fixture: &support::fixture::DaemonFixture,
    backend: WindowsPtyTestBackend,
) {
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(&windows_probe_command("read_line"), true, Some(250), None),
        )
        .await;

    assert!(
        started.running,
        "{} start response: {started:#?}",
        backend.name()
    );

    let session_id = started.daemon_session_id.expect("live session");
    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id.clone(),
                chars: "ping\n".to_string(),
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
            },
        )
        .await;

    let mut combined_output = started.output;
    combined_output.push_str(&response.output);

    let exit_code = if response.running {
        let tail = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: session_id,
                    chars: String::new(),
                    yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                    max_output_tokens: None,
                },
            )
            .await;
        combined_output.push_str(&tail.output);
        tail.exit_code
    } else {
        response.exit_code
    };

    let normalized_output = strip_terminal_noise(&combined_output);

    assert_eq!(
        exit_code,
        Some(0),
        "{} combined output: {:?}",
        backend.name(),
        combined_output
    );
    assert!(
        normalized_output.contains("LINE:ping"),
        "{} combined output did not contain completed line marker: {:?}",
        backend.name(),
        combined_output
    );
}

#[cfg(windows)]
async fn assert_windows_powershell_command_quoting(
    fixture: &support::fixture::DaemonFixture,
    backend: WindowsPtyTestBackend,
) {
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(
                r#"$items = @('plain', 'two words', 'quote "mark"', 'trail\', 'C:\Program Files\Test Folder\'); [Console]::Out.Write(($items -join '|'))"#,
                true,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(
        response.exit_code,
        Some(0),
        "{} response: {response:#?}",
        backend.name()
    );
    assert_eq!(
        strip_terminal_noise(&response.output),
        r#"plain|two words|quote "mark"|trail\|C:\Program Files\Test Folder\"#,
        "{} output: {:?}",
        backend.name(),
        response.output
    );
}

fn process_environment_with(pairs: &[(&str, &str)]) -> ProcessEnvironment {
    let mut environment = ProcessEnvironment::capture_current();
    for (key, value) in pairs {
        environment.set_var(key, Some(OsString::from(value)));
    }
    environment
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
