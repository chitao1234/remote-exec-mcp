use super::*;

const EXPECTED_ENV_OVERLAY_OUTPUT: &str = "dumb|1|cat|cat|1|C|en_US.UTF-8|";
const ENV_OVERLAY_COMMAND: &str = "printf '%s|%s|%s|%s|%s|%s|%s|%s' \"$TERM\" \"$NO_COLOR\" \"$PAGER\" \"$GIT_PAGER\" \"$CODEX_CI\" \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"";

async fn collect_exec_output_until_exit(
    fixture: &support::fixture::DaemonFixture,
    mut response: ExecResponse,
) -> (Option<i32>, String) {
    let mut output = response.output().output.clone();
    for _ in 0..4 {
        if !response.running() {
            return (response.output().exit_code, output);
        }

        let next = fixture
            .rpc::<ExecWriteRequest, ExecResponse>(
                "/v1/exec/write",
                &ExecWriteRequest {
                    daemon_session_id: response
                        .daemon_session_id()
                        .expect("running response should carry daemon_session_id")
                        .to_string(),
                    chars: String::new(),
                    yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                    max_output_tokens: None,
                    pty_size: None,
                },
            )
            .await;
        output.push_str(&next.output().output);
        response = next;
    }

    panic!("exec session did not finish within 4 polls: {response:#?}");
}

#[tokio::test]
async fn exec_start_returns_a_live_session_for_long_running_tty_processes() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("printf ready; sleep 2", true, Some(250), Some(2_000)),
        )
        .await;

    assert!(response.output().running);
    assert!(response.daemon_session_id().is_some());
    assert!(response.output().output.contains("ready"));
}

#[tokio::test]
async fn exec_start_uses_configured_exec_yield_time_policy() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        r#"[yield_time.exec_command]
default_ms = 3000
max_ms = 3000
min_ms = 3000
"#,
    )
    .await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("printf ready; sleep 2", false, Some(1), Some(2_000)),
        )
        .await;

    assert!(!response.output().running, "{response:#?}");
    assert_eq!(response.output().exit_code, Some(0));
    assert!(response.output().output.contains("ready"));
}

#[tokio::test]
async fn exec_start_includes_session_limit_warning_when_threshold_crossed() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", "max_open_sessions = 5").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("printf ready; sleep 2", false, Some(250), Some(2_000)),
        )
        .await;

    assert!(response.output().running, "{response:#?}");
    assert_eq!(response.output().warnings.len(), 1);
    assert_eq!(
        response.output().warnings[0].wire_code(),
        "exec_session_limit_approaching"
    );
    assert_eq!(
        response.output().warnings[0].message,
        "Target `builder-a` now has 1 open exec sessions."
    );
}

#[tokio::test]
async fn exec_start_uses_login_shell_by_default_when_login_is_omitted() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "export LOGIN_SENTINEL=from_profile\n",
    )
    .unwrap();
    let home_text = home.path().to_string_lossy().into_owned();
    let fixture = support::spawn::spawn_daemon_with_process_environment(
        "builder-a",
        process_environment_with(&[("HOME", &home_text), ("SHELL", TEST_SHELL)]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &test_exec_start_request(
                None,
                "printf '%s' \"$LOGIN_SENTINEL\"",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0), "{response:#?}");
    assert!(
        response.output().output.contains("from_profile"),
        "{response:#?}"
    );
}

#[tokio::test]
async fn exec_start_uses_configured_default_shell_when_shell_is_omitted() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_and_process_environment(
        "builder-a",
        &format!(
            "default_shell = {}",
            toml::Value::String(TEST_SHELL.to_string())
        ),
        process_environment_with(&[("SHELL", "/definitely/missing-shell")]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &test_exec_start_request(
                None,
                "printf default-ready",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
                Some(false),
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "default-ready");
}

#[tokio::test]
async fn exec_start_rejects_explicit_login_when_disabled_by_config() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false")
            .await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &unix_start_request_with_login(
                "printf should-not-run",
                false,
                Some(250),
                None,
                Some(true),
            ),
        )
        .await;

    assert_eq!(err.wire_code(), "login_shell_disabled");
    assert!(err.message.contains("login shells are disabled"));
}

#[tokio::test]
async fn exec_start_uses_non_login_shell_when_policy_disabled_and_login_is_omitted() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".profile"),
        "export LOGIN_SENTINEL=from_profile\n",
    )
    .unwrap();
    let home_text = home.path().to_string_lossy().into_owned();
    let fixture = support::spawn::spawn_daemon_with_extra_config_and_process_environment(
        "builder-a",
        "allow_login_shell = false",
        process_environment_with(&[("HOME", &home_text), ("SHELL", TEST_SHELL)]),
    )
    .await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &test_exec_start_request(
                None,
                "printf '%s' \"$LOGIN_SENTINEL\"",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0), "{response:#?}");
    assert_eq!(response.output().output, "");
}

#[tokio::test]
async fn exec_start_rejects_tty_when_disabled_by_config() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", r#"pty = "none""#).await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &unix_start_request("printf should-not-run", true, Some(250), None),
        )
        .await;

    assert_eq!(err.wire_code(), "tty_disabled");
}

#[tokio::test]
async fn exec_start_rejects_cwd_outside_exec_sandbox() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        r#"[sandbox.exec_cwd]
deny = ["/"]
"#,
    )
    .await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &unix_start_request("printf should-not-run", false, Some(250), None),
        )
        .await;

    assert_eq!(err.wire_code(), "sandbox_denied");
    assert!(err.message.contains("exec_cwd access"));
}

#[tokio::test]
async fn env_overlay_is_applied_in_pipe_mode() {
    let fixture = support::spawn::spawn_daemon_with_process_environment(
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
            &unix_start_request(
                ENV_OVERLAY_COMMAND,
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, EXPECTED_ENV_OVERLAY_OUTPUT);
}

#[tokio::test]
async fn env_overlay_is_applied_in_pty_mode() {
    let fixture = support::spawn::spawn_daemon_with_process_environment(
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
            &unix_start_request(
                &format!("{ENV_OVERLAY_COMMAND}; sleep 1"),
                true,
                Some(250),
                None,
            ),
        )
        .await;

    let (exit_code, output) = collect_exec_output_until_exit(&fixture, response).await;
    assert_eq!(exit_code, Some(0));
    assert_eq!(output, EXPECTED_ENV_OVERLAY_OUTPUT);
}

#[tokio::test]
async fn env_overlay_prefers_lang_c_plus_lc_ctype_when_c_utf8_is_unavailable() {
    let fixture = support::spawn::spawn_daemon_with_process_environment(
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
            &unix_start_request(
                "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "C|en_US.UTF-8|");
}

#[tokio::test]
async fn env_overlay_falls_back_to_lang_c_only_when_no_utf8_locale_is_available() {
    let fixture = support::spawn::spawn_daemon_with_process_environment(
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
            &unix_start_request(
                "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "C||");
}

#[tokio::test]
async fn omitted_max_output_tokens_defaults_to_ten_thousand() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "awk 'BEGIN { for (i = 0; i < 25000; ++i) printf \"x \" }'",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().original_token_count, Some(12_500));
    assert!(
        response
            .output()
            .output
            .starts_with("Total output lines: 1\n\n")
    );
    assert!(response.output().output.contains("tokens truncated"));
}

#[tokio::test]
async fn exec_start_truncates_output_to_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "awk 'BEGIN { for (i = 0; i < 100; ++i) printf \"a\" }'",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                Some(15),
            ),
        )
        .await;

    assert_eq!(response.output().original_token_count, Some(25));
    assert_eq!(
        response.output().output,
        "Total output lines: 1\n\naaaaaa\u{2026}22 tokens truncated\u{2026}aaaaaa"
    );
}

#[tokio::test]
async fn exec_output_preserves_trailing_newline_when_within_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "printf 'one two\\n'",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                Some(3),
            ),
        )
        .await;

    assert_eq!(response.output().original_token_count, Some(2));
    assert_eq!(response.output().output, "one two\n");
}

#[tokio::test]
async fn exec_output_drains_late_output_after_exit() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "(sleep 0.08; printf 'late tail') &",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                Some(10),
            ),
        )
        .await;

    assert!(!response.output().running);
    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "late tail");
}

#[tokio::test]
async fn exec_output_preserves_pipe_mode_output_after_external_pipeline_steps() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "printf 'marker\\n'; printf 'external\\n' | cat; printf 'done\\n'",
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert!(!response.output().running);
    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "marker\nexternal\ndone\n");
}

#[tokio::test]
async fn exec_output_uses_one_pipe_for_stdout_and_stderr_in_pipe_mode() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                r#"if [ "$(readlink /proc/$$/fd/1)" = "$(readlink /proc/$$/fd/2)" ]; then printf 'shared\n'; else printf 'separate\n'; fi"#,
                false,
                Some(COMPLETED_COMMAND_YIELD_MS),
                None,
            ),
        )
        .await;

    assert!(!response.output().running);
    assert_eq!(response.output().exit_code, Some(0));
    assert_eq!(response.output().output, "shared\n");
}

#[tokio::test]
async fn exec_empty_poll_truncates_pty_output_to_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "sleep 0.4; printf 'one two three four five six'; sleep 30",
                true,
                Some(250),
                Some(3),
            ),
        )
        .await;
    assert!(started.output().running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started
                    .daemon_session_id()
                    .expect("live session")
                    .to_string(),
                chars: "".to_string(),
                yield_time_ms: Some(5_000),
                max_output_tokens: Some(3),
                pty_size: None,
            },
        )
        .await;

    assert!(response.output().running);
    assert_eq!(response.output().original_token_count, Some(7));
    assert_eq!(
        response.output().output,
        "Total output lines: 1\n\n\u{2026}7 tokens truncated\u{2026}"
    );
}

#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("sleep 1", false, Some(250), Some(2_000)),
        )
        .await;

    let session_id = started
        .daemon_session_id()
        .expect("live session")
        .to_string();
    let err = fixture
        .rpc_error(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id,
                chars: "pwd\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                pty_size: None,
            },
        )
        .await;

    assert_eq!(err.wire_code(), "stdin_closed");
    assert!(err.message.contains("tty=true"));
}

#[tokio::test]
async fn exec_write_round_trips_pty_input_without_echo_assumptions() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "IFS= read -r line; printf '__RESULT__:%s:__END__' \"$line\"",
                true,
                Some(250),
                None,
            ),
        )
        .await;

    assert!(started.output().running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started
                    .daemon_session_id()
                    .expect("live session")
                    .to_string(),
                chars: "ping pong\n".to_string(),
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                pty_size: None,
            },
        )
        .await;

    assert_eq!(response.output().exit_code, Some(0));
    assert!(
        response
            .output()
            .output
            .contains("__RESULT__:ping pong:__END__")
    );
}

#[tokio::test]
async fn exec_write_resizes_pty_before_polling_output() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "printf ready; IFS= read -r _; stty size; sleep 30",
                true,
                Some(250),
                None,
            ),
        )
        .await;
    assert!(started.output().running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started
                    .daemon_session_id()
                    .expect("live session")
                    .to_string(),
                chars: "\n".to_string(),
                yield_time_ms: Some(2_000),
                max_output_tokens: None,
                pty_size: Some(remote_exec_proto::rpc::ExecPtySize {
                    rows: 33,
                    cols: 101,
                }),
            },
        )
        .await;

    assert!(response.output().running);
    assert!(
        response.output().output.contains("33 101"),
        "PTY size output did not include resized dimensions: {:?}",
        response.output().output
    );
}

#[tokio::test]
async fn exec_write_rejects_zero_pty_size() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("sleep 30", true, Some(250), None),
        )
        .await;
    assert!(started.output().running);

    let err = fixture
        .rpc_error(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started
                    .daemon_session_id()
                    .expect("live session")
                    .to_string(),
                chars: String::new(),
                yield_time_ms: Some(250),
                max_output_tokens: None,
                pty_size: Some(remote_exec_proto::rpc::ExecPtySize { rows: 0, cols: 80 }),
            },
        )
        .await;

    assert_eq!(err.wire_code(), "invalid_pty_size");
    assert!(err.message.contains("greater than zero"));
}

#[tokio::test]
async fn exec_write_does_not_block_unrelated_sessions_on_same_daemon() {
    use std::time::{Duration, Instant};

    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let slow = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request("printf slow; sleep 30", true, Some(250), None),
        )
        .await;
    let fast = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &unix_start_request(
                "read line; printf '%s' \"$line\"; sleep 30",
                true,
                Some(250),
                None,
            ),
        )
        .await;

    let slow_client = fixture.client.clone();
    let slow_url = fixture.url("/v1/exec/write");
    let slow_session_id = slow.daemon_session_id().expect("slow session").to_string();
    let slow_poll = tokio::spawn(async move {
        slow_client
            .post(slow_url)
            .json(&ExecWriteRequest {
                daemon_session_id: slow_session_id,
                chars: "".to_string(),
                yield_time_ms: Some(5_000),
                max_output_tokens: None,
                pty_size: None,
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
                daemon_session_id: fast
                    .daemon_session_id()
                    .expect("fast session")
                    .to_string()
                    .to_string(),
                chars: "ping\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: None,
                pty_size: None,
            },
        )
        .await;

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "fast session waited behind unrelated session for {:?}",
        started.elapsed()
    );
    assert!(fast_response.output().output.contains("ping"));

    let _ = slow_poll.await.unwrap();
}
