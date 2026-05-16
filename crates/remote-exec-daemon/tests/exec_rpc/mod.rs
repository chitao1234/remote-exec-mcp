#[path = "../support/mod.rs"]
mod support;

#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use remote_exec_daemon::config::ProcessEnvironment;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest};
#[cfg(windows)]
use support::spawn::WindowsPtyTestBackend;
use support::test_helpers::DEFAULT_TEST_TARGET;

#[cfg(unix)]
const TEST_SHELL: &str = "/bin/sh";
#[cfg(windows)]
const TEST_SHELL: &str = "powershell.exe";
#[cfg(windows)]
const WINDOWS_CMD_SHELL: &str = "cmd.exe";
#[cfg(windows)]
const WINDOWS_GIT_BASH_COMMON_PATH: &str = r"C:\Program Files\Git\bin\bash.exe";
#[cfg(windows)]
const WINDOWS_ENV_OVERLAY_OUTPUT: &str = "dumb|1|cat|cat|1|C.UTF-8|C.UTF-8|C.UTF-8";
// Commands in these tests are expected to finish in a single RPC response, but the daemon only
// guarantees a minimum 250 ms wait and may legitimately return a live session under heavy load.
// Use a generous window so large-output assertions do not become timing-sensitive.
const COMPLETED_COMMAND_YIELD_MS: u64 = 10_000;

fn test_exec_start_request(
    shell: Option<&str>,
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    login: Option<bool>,
) -> ExecStartRequest {
    ExecStartRequest {
        cmd: cmd.to_string(),
        workdir: None,
        shell: shell.map(str::to_owned),
        tty,
        yield_time_ms,
        max_output_tokens,
        login,
    }
}

#[cfg(unix)]
fn unix_start_request(
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
) -> ExecStartRequest {
    unix_start_request_with_login(cmd, tty, yield_time_ms, max_output_tokens, Some(false))
}

#[cfg(unix)]
fn unix_start_request_with_login(
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    login: Option<bool>,
) -> ExecStartRequest {
    test_exec_start_request(
        Some(TEST_SHELL),
        cmd,
        tty,
        yield_time_ms,
        max_output_tokens,
        login,
    )
}

#[cfg(windows)]
macro_rules! for_each_windows_pty_backend {
    ($backend:ident, $fixture:ident, $body:block) => {{
        for $backend in support::spawn::supported_windows_pty_backends() {
            let $fixture =
                support::spawn::spawn_daemon_for_windows_pty_backend(DEFAULT_TEST_TARGET, $backend)
                    .await;
            $body
        }
    }};
}

#[cfg(windows)]
macro_rules! for_each_windows_pty_backend_with_environment {
    ($backend:ident, $fixture:ident, $environment:expr, $body:block) => {{
        for $backend in support::spawn::supported_windows_pty_backends() {
            let $fixture =
                support::spawn::spawn_daemon_for_windows_pty_backend_with_process_environment(
                    DEFAULT_TEST_TARGET,
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
    windows_start_request_with_shell(
        Some(TEST_SHELL),
        cmd,
        tty,
        yield_time_ms,
        max_output_tokens,
        false,
    )
}

#[cfg(windows)]
fn windows_start_request_with_shell(
    shell: Option<&str>,
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    login: bool,
) -> ExecStartRequest {
    test_exec_start_request(
        shell,
        cmd,
        tty,
        yield_time_ms,
        max_output_tokens,
        Some(login),
    )
}

#[cfg(windows)]
fn windows_cmd_start_request(
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
) -> ExecStartRequest {
    windows_start_request_with_shell(
        Some(WINDOWS_CMD_SHELL),
        cmd,
        tty,
        yield_time_ms,
        max_output_tokens,
        false,
    )
}

#[cfg(windows)]
fn windows_git_bash_start_request(
    shell: Option<&str>,
    cmd: &str,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    login: bool,
) -> ExecStartRequest {
    windows_start_request_with_shell(shell, cmd, tty, yield_time_ms, max_output_tokens, login)
}

#[cfg(windows)]
fn windows_probe_command(mode: &str) -> String {
    let probe = Path::new(env!("CARGO_BIN_EXE_pty_input_probe"))
        .display()
        .to_string();
    format!("& '{probe}' {mode}")
}

#[cfg(windows)]
fn available_windows_git_bash_path() -> Option<String> {
    let mut candidates = vec![PathBuf::from(WINDOWS_GIT_BASH_COMMON_PATH)];
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(
            PathBuf::from(program_files_x86)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(local_app_data) = std::env::var_os("LocalAppData") {
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(git_path) =
        find_windows_command_on_path(std::env::var_os("PATH").as_deref(), "git.exe")
    {
        for ancestor in git_path.ancestors().skip(1).take(4) {
            candidates.push(ancestor.join("bin").join("bash.exe"));
            candidates.push(ancestor.join("usr").join("bin").join("bash.exe"));
        }
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn windows_bash_root(shell: &str) -> Option<PathBuf> {
    Path::new(shell)
        .ancestors()
        .skip(1)
        .take(4)
        .find(|ancestor| {
            ancestor.join("bin").join("bash.exe").is_file()
                || ancestor.join("usr").join("bin").join("bash.exe").is_file()
        })
        .map(Path::to_path_buf)
}

#[cfg(windows)]
fn find_windows_command_on_path(path_env: Option<&OsStr>, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_env?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
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
        started.output().running,
        "{} start response: {started:#?}",
        backend.name()
    );
    let session_id = started
        .daemon_session_id()
        .expect("tty start should create a live session");

    let polled = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id.to_string(),
                chars: String::new(),
                yield_time_ms: Some(5_000),
                max_output_tokens: Some(2_000),
                pty_size: None,
            },
        )
        .await;

    assert!(
        polled.output().running,
        "{} poll response: {polled:#?}",
        backend.name()
    );

    let combined_output = format!("{}{}", started.output().output, polled.output().output);
    let normalized_output = combined_output.to_ascii_lowercase();
    assert!(
        normalized_output.contains("hello"),
        "{} combined output did not contain hello: {combined_output:?}",
        backend.name()
    );
    assert!(
        !combined_output.contains('\u{1b}'),
        "{} combined output leaked ESC control sequence: {combined_output:?}",
        backend.name()
    );
    assert!(
        !combined_output.contains('\u{7}'),
        "{} combined output leaked BEL control sequence: {combined_output:?}",
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
        started.output().running,
        "{} start response: {started:#?}",
        backend.name()
    );

    let session_id = started
        .daemon_session_id()
        .expect("live session")
        .to_string();
    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id.clone(),
                chars: "ping\n".to_string(),
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                pty_size: None,
            },
        )
        .await;

    let mut combined_output = started.output().output.clone();
    combined_output.push_str(&response.output().output);

    let exit_code = if response.output().running {
        let tail = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: session_id,
                    chars: String::new(),
                    yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                    max_output_tokens: None,
                    pty_size: None,
                },
            )
            .await;
        combined_output.push_str(&tail.output().output);
        tail.output().exit_code
    } else {
        response.output().exit_code
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
        response.output().exit_code,
        Some(0),
        "{} response: {response:#?}",
        backend.name()
    );
    assert_eq!(
        strip_terminal_noise(&response.output().output),
        r#"plain|two words|quote "mark"|trail\|C:\Program Files\Test Folder\"#,
        "{} output: {:?}",
        backend.name(),
        response.output().output
    );
}

#[cfg(windows)]
async fn assert_windows_git_bash_tty_read_line(
    fixture: &support::fixture::DaemonFixture,
    backend: WindowsPtyTestBackend,
    shell: Option<&str>,
) {
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_git_bash_start_request(
                shell,
                "printf 'READY\\n'; IFS= read -r line; printf 'LINE:%s\\n' \"$line\"",
                true,
                Some(250),
                None,
                false,
            ),
        )
        .await;

    assert!(
        started.output().running,
        "{} start response: {started:#?}",
        backend.name()
    );
    let session_id = started
        .daemon_session_id()
        .expect("live session")
        .to_string();
    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id.clone(),
                chars: "ping\n".to_string(),
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                pty_size: None,
            },
        )
        .await;

    let mut combined_output = started.output().output.clone();
    combined_output.push_str(&response.output().output);

    let exit_code = if response.output().running {
        let tail = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: session_id,
                    chars: String::new(),
                    yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                    max_output_tokens: None,
                    pty_size: None,
                },
            )
            .await;
        combined_output.push_str(&tail.output().output);
        tail.output().exit_code
    } else {
        response.output().exit_code
    };

    let normalized_output = strip_terminal_noise(&combined_output).replace('\r', "");

    assert_eq!(
        exit_code,
        Some(0),
        "{} combined output: {:?}",
        backend.name(),
        combined_output
    );
    assert!(
        normalized_output.contains("READY"),
        "{} combined output missing READY marker: {:?}",
        backend.name(),
        combined_output
    );
    assert!(
        normalized_output.contains("LINE:ping"),
        "{} combined output missing line echo: {:?}",
        backend.name(),
        combined_output
    );
}

#[cfg(windows)]
async fn assert_windows_git_bash_command_quoting(
    fixture: &support::fixture::DaemonFixture,
    backend: WindowsPtyTestBackend,
    shell: Option<&str>,
) {
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_git_bash_start_request(
                shell,
                "first='two words'; second='quote \"mark\"'; third='literal $HOME & | ; * ?'; fourth='C:\\Program Files\\Test Folder\\'; printf '%s' \"$first|$second|$third|$fourth\"",
                true,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
                false,
            ),
        )
        .await;

    assert_eq!(
        response.output().exit_code,
        Some(0),
        "{} response: {response:#?}",
        backend.name()
    );
    assert_eq!(
        strip_terminal_noise(&response.output().output).replace('\r', ""),
        r#"two words|quote "mark"|literal $HOME & | ; * ?|C:\Program Files\Test Folder\"#,
        "{} output: {:?}",
        backend.name(),
        response.output().output
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
