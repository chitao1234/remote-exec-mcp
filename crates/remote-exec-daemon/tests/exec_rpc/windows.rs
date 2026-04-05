use super::*;

#[tokio::test]
async fn exec_start_allows_login_requests_on_windows_when_enabled() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_start_rejects_login_requests_on_windows_when_disabled_by_config() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", "allow_login_shell = false")
            .await;

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

#[tokio::test]
async fn exec_start_uses_configured_default_shell_when_shell_is_omitted() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_and_process_environment(
        "builder-a",
        r#"default_shell = "cmd.exe""#,
        process_environment_with(&[("PATH", ""), ("COMSPEC", "missing-cmd.exe")]),
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

#[tokio::test]
async fn exec_start_prefers_git_bash_when_shell_is_omitted_on_windows() {
    let Some(_git_bash) = available_windows_git_bash_path() else {
        return;
    };

    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s' git-bash-ready".to_string(),
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
    assert_eq!(response.output, "git-bash-ready");
}

#[tokio::test]
async fn exec_start_resolves_bare_bash_shell_requests_to_git_bash_on_windows() {
    let Some(_git_bash) = available_windows_git_bash_path() else {
        return;
    };

    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s' explicit-git-bash".to_string(),
                workdir: None,
                shell: Some("bash.exe".to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "explicit-git-bash");
}

#[tokio::test]
async fn exec_start_preserves_workdir_for_git_bash_login_shells_on_windows() {
    let Some(_git_bash) = available_windows_git_bash_path() else {
        return;
    };

    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let workdir = fixture.workdir.join("git bash cwd");
    std::fs::create_dir_all(&workdir).unwrap();

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s' \"$(pwd -W)\"".to_string(),
                workdir: Some(workdir.display().to_string()),
                shell: Some("bash.exe".to_string()),
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: Some(true),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(
        response.output.replace('\\', "/"),
        workdir.display().to_string().replace('\\', "/")
    );
}

#[tokio::test]
async fn exec_start_uses_git_bash_login_profiles_when_shell_is_omitted() {
    let Some(_git_bash) = available_windows_git_bash_path() else {
        return;
    };

    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".bash_profile"),
        "export LOGIN_SENTINEL=from_git_bash_profile\n",
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
                shell: None,
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "from_git_bash_profile");
}

#[tokio::test]
async fn exec_start_rejects_tty_when_disabled_by_config_on_windows() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config("builder-a", r#"pty = "none""#).await;

    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo should-not-run".to_string(),
                workdir: None,
                shell: Some("cmd.exe".to_string()),
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
async fn exec_start_keeps_windows_tty_sessions_alive_and_answers_terminal_queries() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_tty_session_answers_terminal_queries(&fixture, backend).await;
    });
}

#[tokio::test]
async fn env_overlay_is_applied_in_pipe_mode_on_windows() {
    let fixture = support::spawn::spawn_daemon_with_process_environment(
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

#[tokio::test]
async fn omitted_max_output_tokens_defaults_to_ten_thousand_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_start_truncates_output_to_max_output_tokens_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_output_preserves_trailing_newline_when_within_max_output_tokens_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[tokio::test]
async fn exec_write_bare_lf_advances_windows_pty_line_reader() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_bare_lf_advances_pty_line_reader(&fixture, backend).await;
    });
}

#[tokio::test]
async fn exec_start_preserves_complex_powershell_command_quoting_across_windows_pty_backends() {
    for_each_windows_pty_backend!(backend, fixture, {
        assert_windows_powershell_command_quoting(&fixture, backend).await;
    });
}
