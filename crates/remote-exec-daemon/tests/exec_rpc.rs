mod support;

use std::ffi::OsString;
use std::sync::OnceLock;

#[cfg(unix)]
use remote_exec_proto::rpc::ExecWriteRequest;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest};
use tokio::sync::Mutex;

#[cfg(unix)]
const TEST_SHELL: &str = "/bin/sh";
// Commands in these tests are expected to finish in a single RPC response, but the daemon only
// guarantees a minimum 250 ms wait. Use a wider window so full-suite load does not turn them into
// legitimate live-session responses.
const COMPLETED_COMMAND_YIELD_MS: u64 = 5_000;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvOverrideGuard {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    saved: Vec<(String, Option<OsString>)>,
}

impl EnvOverrideGuard {
    async fn set(pairs: &[(&str, &str)]) -> Self {
        let guard = env_lock().lock().await;
        let mut saved = Vec::new();

        for (key, value) in pairs {
            saved.push((key.to_string(), std::env::var_os(key)));
            unsafe {
                std::env::set_var(key, value);
            }
        }

        Self {
            _guard: guard,
            saved,
        }
    }
}

impl Drop for EnvOverrideGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
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
    let _env = EnvOverrideGuard::set(&[("HOME", &home_text)]).await;
    let fixture = support::spawn_daemon("builder-a").await;

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
    let _env = EnvOverrideGuard::set(&[("PATH", ""), ("COMSPEC", "cmd.exe")]).await;
    let fixture = support::spawn_daemon("builder-a").await;

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
    let _env = EnvOverrideGuard::set(&[("HOME", &home_text)]).await;
    let fixture =
        support::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false").await;

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
    let _env = EnvOverrideGuard::set(&[
        ("TERM", "rainbow-terminal"),
        ("NO_COLOR", "0"),
        ("PAGER", "less"),
        ("GIT_PAGER", "more"),
        ("CODEX_CI", "0"),
        (
            "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
            "fr_FR.UTF-8\nen_US.UTF-8\n",
        ),
    ])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

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
    let _env = EnvOverrideGuard::set(&[
        ("TERM", "rainbow-terminal"),
        ("NO_COLOR", "0"),
        ("PAGER", "less"),
        ("GIT_PAGER", "more"),
        ("CODEX_CI", "0"),
        (
            "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
            "fr_FR.UTF-8\nen_US.UTF-8\n",
        ),
    ])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

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
    let _env = EnvOverrideGuard::set(&[(
        "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
        "fr_FR.UTF-8\nen_US.UTF-8\n",
    )])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

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
    let _env = EnvOverrideGuard::set(&[(
        "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
        "C\nPOSIX\nen_US.ISO8859-1\n",
    )])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

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
