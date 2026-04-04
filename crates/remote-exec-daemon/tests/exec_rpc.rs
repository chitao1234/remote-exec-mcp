mod support;

use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;

use remote_exec_daemon::config::ProcessEnvironment;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest};
#[cfg(windows)]
use support::WindowsPtyTestBackend;

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
macro_rules! for_each_windows_pty_backend {
    ($backend:ident, $fixture:ident, $body:block) => {{
        for $backend in support::supported_windows_pty_backends() {
            let $fixture =
                support::spawn_daemon_for_windows_pty_backend("builder-a", $backend).await;
            $body
        }
    }};
}

#[cfg(windows)]
macro_rules! for_each_windows_pty_backend_with_environment {
    ($backend:ident, $fixture:ident, $environment:expr, $body:block) => {{
        for $backend in support::supported_windows_pty_backends() {
            let $fixture = support::spawn_daemon_for_windows_pty_backend_with_process_environment(
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
    fixture: &support::DaemonFixture,
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
    fixture: &support::DaemonFixture,
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
    fixture: &support::DaemonFixture,
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
#[tokio::test]
async fn exec_start_returns_a_live_session_for_long_running_tty_processes() {
    let fixture = support::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf ready; sleep 2".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    assert!(response.running);
    assert!(response.daemon_session_id.is_some());
    assert!(response.output.contains("ready"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_start_uses_login_shell_by_default_when_login_is_omitted() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "export LOGIN_SENTINEL=from_profile\n",
    )
    .unwrap();
    let home_text = home.path().to_string_lossy().into_owned();
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[("HOME", &home_text)]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s' \"$LOGIN_SENTINEL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "from_profile");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_start_rejects_explicit_login_when_disabled_by_config() {
    let fixture =
        support::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false").await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf should-not-run".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(true),
            },
        )
        .await;

    assert_eq!(err.code, "login_shell_disabled");
    assert!(err.message.contains("login shells are disabled"));
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_allows_login_requests_on_windows_when_enabled() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo windows-ready".to_string(),
                workdir: None,
                shell: Some("cmd.exe".to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(true),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(
        response
            .output
            .to_ascii_lowercase()
            .contains("windows-ready")
    );
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_rejects_login_requests_on_windows_when_disabled_by_config() {
    let fixture =
        support::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false").await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo should-not-run".to_string(),
                workdir: None,
                shell: Some("cmd.exe".to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(true),
            },
        )
        .await;

    assert_eq!(err.code, "login_shell_disabled");
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_uses_cmd_when_shell_is_omitted() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[("PATH", ""), ("COMSPEC", "cmd.exe")]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo windows-ready".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(
        response
            .output
            .to_ascii_lowercase()
            .contains("windows-ready")
    );
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_keeps_windows_tty_sessions_alive_and_answers_terminal_queries() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_tty_session_answers_terminal_queries(&fixture, backend).await;
    });
}

#[cfg(windows)]
#[tokio::test]
async fn env_overlay_is_applied_in_pipe_mode_on_windows() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[
            ("TERM", "rainbow-terminal"),
            ("NO_COLOR", "0"),
            ("PAGER", "less"),
            ("GIT_PAGER", "more"),
            ("CODEX_CI", "0"),
            ("LANG", "fr_FR.UTF-8"),
            ("LC_CTYPE", "fr_FR.UTF-8"),
            ("LC_ALL", "fr_FR.UTF-8"),
        ]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(
                r#"[Console]::Out.Write("$env:TERM|$env:NO_COLOR|$env:PAGER|$env:GIT_PAGER|$env:CODEX_CI|$env:LANG|$env:LC_CTYPE|$env:LC_ALL")"#,
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(
        response.output.ends_with(WINDOWS_ENV_OVERLAY_OUTPUT),
        "unexpected pty output: {:?}",
        response.output
    );
}

#[cfg(windows)]
#[tokio::test]
async fn env_overlay_is_applied_in_pty_mode_on_windows() {
    let environment = process_environment_with(&[
        ("TERM", "rainbow-terminal"),
        ("NO_COLOR", "0"),
        ("PAGER", "less"),
        ("GIT_PAGER", "more"),
        ("CODEX_CI", "0"),
        ("LANG", "fr_FR.UTF-8"),
        ("LC_CTYPE", "fr_FR.UTF-8"),
        ("LC_ALL", "fr_FR.UTF-8"),
    ]);
    for_each_windows_pty_backend_with_environment!(backend, fixture, environment, {
        let response = fixture
            .rpc::<ExecStartRequest, ExecResponse>(
                "/v1/exec/start",
                &windows_start_request(
                    r#"[Console]::Out.Write("$env:TERM|$env:NO_COLOR|$env:PAGER|$env:GIT_PAGER|$env:CODEX_CI|$env:LANG|$env:LC_CTYPE|$env:LC_ALL")"#,
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
        let normalized_output = strip_terminal_noise(&response.output);
        assert!(
            normalized_output.ends_with(WINDOWS_ENV_OVERLAY_OUTPUT),
            "{} unexpected pty output: {:?}",
            backend.name(),
            response.output
        );
    });
}

#[cfg(windows)]
#[tokio::test]
async fn omitted_max_output_tokens_defaults_to_ten_thousand_on_windows() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(
                "[Console]::Out.Write(('x ' * 10005))",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.original_token_count, Some(10_005));
    assert_eq!(response.output.split_whitespace().count(), 10_000);
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_truncates_output_to_max_output_tokens_on_windows() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(
                "[Console]::Out.Write('one two three four five six')",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                Some(3),
            ),
        )
        .await;

    assert_eq!(response.original_token_count, Some(6));
    assert_eq!(response.output, "one two three");
}

#[cfg(windows)]
#[tokio::test]
async fn exec_output_preserves_trailing_newline_when_within_max_output_tokens_on_windows() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request(
                r#"[Console]::Out.Write("one two`n")"#,
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                Some(3),
            ),
        )
        .await;

    assert_eq!(response.original_token_count, Some(2));
    assert_eq!(response.output, "one two\n");
}

#[cfg(windows)]
#[tokio::test]
async fn exec_empty_poll_truncates_pty_output_to_max_output_tokens_on_windows() {
    for_each_windows_pty_backend!(backend, fixture, {
        let started = fixture
            .rpc::<ExecStartRequest, ExecResponse>(
                "/v1/exec/start",
                &windows_start_request(
                    &windows_probe_command("delayed_tokens"),
                    true,
                    Some(250),
                    Some(3),
                ),
            )
            .await;
        assert!(
            started.running,
            "{} start response: {started:#?}",
            backend.name()
        );

        let response = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: started.daemon_session_id.expect("live session"),
                    chars: String::new(),
                    yield_time_ms: Some(5_000),
                    max_output_tokens: Some(3),
                },
            )
            .await;

        assert!(
            response.running,
            "{} poll response: {response:#?}",
            backend.name()
        );
        assert_eq!(
            response.original_token_count,
            Some(6),
            "{} response: {response:#?}",
            backend.name()
        );
        assert_eq!(
            strip_terminal_noise(&response.output),
            "one two three",
            "{} response: {response:#?}",
            backend.name()
        );
    });
}

#[cfg(windows)]
#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present_on_windows() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &windows_start_request("Start-Sleep -Seconds 1", false, Some(250), Some(2_000)),
        )
        .await;

    let session_id = started.daemon_session_id.expect("live session");
    let err = fixture
        .rpc_error(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id,
                chars: "pwd\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
            },
        )
        .await;

    assert_eq!(err.code, "stdin_closed");
    assert!(err.message.contains("tty=true"));
}

#[cfg(windows)]
#[tokio::test]
async fn exec_write_does_not_block_unrelated_sessions_on_same_daemon_on_windows() {
    use std::time::{Duration, Instant};

    for_each_windows_pty_backend!(backend, fixture, {
        let slow = fixture
            .rpc::<ExecStartRequest, ExecResponse>(
                "/v1/exec/start",
                &windows_cmd_start_request(
                    "echo slow & ping -n 30 127.0.0.1 >nul",
                    true,
                    Some(250),
                    None,
                ),
            )
            .await;
        let fast = fixture
            .rpc::<ExecStartRequest, ExecResponse>(
                "/v1/exec/start",
                &windows_cmd_start_request(
                    "setlocal EnableDelayedExpansion & set /P line= & echo !line! & ping -n 30 127.0.0.1 >nul",
                    true,
                    Some(250),
                    None,
                ),
            )
            .await;

        let slow_client = fixture.client.clone();
        let slow_url = fixture.url("/v1/exec/write");
        let slow_session_id = slow.daemon_session_id.clone().expect("slow session");
        let slow_poll = tokio::spawn(async move {
            slow_client
                .post(slow_url)
                .json(&ExecWriteRequest {
                    daemon_session_id: slow_session_id,
                    chars: String::new(),
                    yield_time_ms: Some(5_000),
                    max_output_tokens: None,
                })
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<ExecResponse>()
                .await
                .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let started = Instant::now();
        let fast_response = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: fast.daemon_session_id.expect("fast session"),
                    chars: "ping\n".to_string(),
                    yield_time_ms: Some(250),
                    max_output_tokens: None,
                },
            )
            .await;

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{} fast session waited behind unrelated session for {:?}",
            backend.name(),
            started.elapsed()
        );
        assert!(
            fast_response.output.contains("ping"),
            "{} fast response: {fast_response:#?}",
            backend.name()
        );

        let _ = slow_poll.await.unwrap();
    });
}

#[cfg(windows)]
#[tokio::test]
async fn exec_write_bare_lf_advances_windows_pty_line_reader() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_bare_lf_advances_pty_line_reader(&fixture, backend).await;
    });
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_preserves_complex_powershell_command_quoting_across_windows_pty_backends() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_powershell_command_quoting(&fixture, backend).await;
    });
}

#[cfg(unix)]
#[tokio::test]
async fn exec_start_uses_non_login_shell_when_policy_disabled_and_login_is_omitted() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "export LOGIN_SENTINEL=from_profile\n",
    )
    .unwrap();
    let home_text = home.path().to_string_lossy().into_owned();
    let fixture = support::spawn_daemon_with_extra_config_and_process_environment(
        "builder-a",
        "allow_login_shell = false",
        process_environment_with(&[("HOME", &home_text)]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s' \"$LOGIN_SENTINEL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "");
}

#[cfg(unix)]
#[tokio::test]
async fn env_overlay_is_applied_in_pipe_mode() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[
            ("TERM", "rainbow-terminal"),
            ("NO_COLOR", "0"),
            ("PAGER", "less"),
            ("GIT_PAGER", "more"),
            ("CODEX_CI", "0"),
            (
                "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
                "fr_FR.UTF-8\nen_US.UTF-8\n",
            ),
        ]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s|%s|%s|%s|%s|%s' \"$TERM\" \"$NO_COLOR\" \"$PAGER\" \"$GIT_PAGER\" \"$CODEX_CI\" \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\""
                    .to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "dumb|1|cat|cat|1|C|en_US.UTF-8|");
}

#[cfg(unix)]
#[tokio::test]
async fn env_overlay_is_applied_in_pty_mode() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[
            ("TERM", "rainbow-terminal"),
            ("NO_COLOR", "0"),
            ("PAGER", "less"),
            ("GIT_PAGER", "more"),
            ("CODEX_CI", "0"),
            (
                "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
                "fr_FR.UTF-8\nen_US.UTF-8\n",
            ),
        ]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s|%s|%s|%s|%s|%s' \"$TERM\" \"$NO_COLOR\" \"$PAGER\" \"$GIT_PAGER\" \"$CODEX_CI\" \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\""
                    .to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "dumb|1|cat|cat|1|C|en_US.UTF-8|");
}

#[cfg(unix)]
#[tokio::test]
async fn env_overlay_prefers_lang_c_plus_lc_ctype_when_c_utf8_is_unavailable() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[(
            "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
            "fr_FR.UTF-8\nen_US.UTF-8\n",
        )]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "C|en_US.UTF-8|");
}

#[cfg(unix)]
#[tokio::test]
async fn env_overlay_falls_back_to_lang_c_only_when_no_utf8_locale_is_available() {
    let fixture = support::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[(
            "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
            "C\nPOSIX\nen_US.ISO8859-1\n",
        )]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "C||");
}

#[cfg(unix)]
#[tokio::test]
async fn omitted_max_output_tokens_defaults_to_ten_thousand() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "awk 'BEGIN { for (i = 0; i < 10005; ++i) printf \"x \" }'".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.original_token_count, Some(10_005));
    assert_eq!(response.output.split_whitespace().count(), 10_000);
}

#[cfg(unix)]
#[tokio::test]
async fn exec_start_truncates_output_to_max_output_tokens() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf 'one two three four five six'".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: Some(3),
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.original_token_count, Some(6));
    assert_eq!(response.output, "one two three");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_output_preserves_trailing_newline_when_within_max_output_tokens() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf 'one two\\n'".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: Some(3),
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.original_token_count, Some(2));
    assert_eq!(response.output, "one two\n");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_output_drains_late_output_after_exit() {
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "(sleep 0.08; printf 'late tail') &".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: Some(10),
                login: Some(false),
            },
        )
        .await;

    assert!(!response.running);
    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "late tail");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_empty_poll_truncates_pty_output_to_max_output_tokens() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "sleep 0.4; printf 'one two three four five six'; sleep 30".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: Some(3),
                login: Some(false),
            },
        )
        .await;
    assert!(started.running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started.daemon_session_id.expect("live session"),
                chars: "".to_string(),
                yield_time_ms: Some(5_000),
                max_output_tokens: Some(3),
            },
        )
        .await;

    assert!(response.running);
    assert_eq!(response.original_token_count, Some(6));
    assert_eq!(response.output, "one two three");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "sleep 1".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    let session_id = started.daemon_session_id.expect("live session");
    let err = fixture
        .rpc_error(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id,
                chars: "pwd\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
            },
        )
        .await;

    assert_eq!(err.code, "stdin_closed");
    assert!(err.message.contains("tty=true"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_write_round_trips_pty_input_without_echo_assumptions() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "IFS= read -r line; printf '__RESULT__:%s:__END__' \"$line\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert!(started.running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started.daemon_session_id.expect("live session"),
                chars: "ping pong\n".to_string(),
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(response.output.contains("__RESULT__:ping pong:__END__"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_write_does_not_block_unrelated_sessions_on_same_daemon() {
    use std::time::{Duration, Instant};

    let fixture = support::spawn_daemon("builder-a").await;

    let slow = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf slow; sleep 30".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;
    let fast = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "read line; printf '%s' \"$line\"; sleep 30".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    let slow_client = fixture.client.clone();
    let slow_url = fixture.url("/v1/exec/write");
    let slow_session_id = slow.daemon_session_id.clone().expect("slow session");
    let slow_poll = tokio::spawn(async move {
        slow_client
            .post(slow_url)
            .json(&ExecWriteRequest {
                daemon_session_id: slow_session_id,
                chars: "".to_string(),
                yield_time_ms: Some(5_000),
                max_output_tokens: None,
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<ExecResponse>()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    let fast_response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: fast.daemon_session_id.expect("fast session"),
                chars: "ping\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: None,
            },
        )
        .await;

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "fast session waited behind unrelated session for {:?}",
        started.elapsed()
    );
    assert!(fast_response.output.contains("ping"));

    let _ = slow_poll.await.unwrap();
}
