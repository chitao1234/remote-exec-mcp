use super::*;

#[tokio::test]
async fn exec_start_returns_a_live_session_for_long_running_tty_processes() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
            &ExecStartRequest {
                cmd: "printf ready; sleep 2".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(1),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    assert!(!response.running, "{response:#?}");
    assert_eq!(response.exit_code, Some(0));
    assert!(response.output.contains("ready"));
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
            &ExecStartRequest {
                cmd: "printf default-ready".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "default-ready");
}

#[tokio::test]
async fn exec_start_rejects_explicit_login_when_disabled_by_config() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false")
            .await;

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

#[tokio::test]
async fn exec_start_rejects_tty_when_disabled_by_config() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", r#"pty = "none""#).await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf should-not-run".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(err.code, "tty_disabled");
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
            &ExecStartRequest {
                cmd: "printf should-not-run".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(err.code, "sandbox_denied");
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

#[tokio::test]
async fn omitted_max_output_tokens_defaults_to_ten_thousand() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_start_truncates_output_to_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_output_preserves_trailing_newline_when_within_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_output_drains_late_output_after_exit() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_empty_poll_truncates_pty_output_to_max_output_tokens() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[tokio::test]
async fn exec_write_round_trips_pty_input_without_echo_assumptions() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[tokio::test]
async fn exec_write_does_not_block_unrelated_sessions_on_same_daemon() {
    use std::time::{Duration, Instant};

    let fixture = support::spawn::spawn_daemon("builder-a").await;

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
