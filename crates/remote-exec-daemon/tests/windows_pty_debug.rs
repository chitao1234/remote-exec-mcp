//! Manual Windows PTY diagnostics.
//!
//! These tests are intentionally ignored and Windows-only. They are kept under
//! `tests/` so developers can run them with `cargo test -p remote-exec-daemon
//! --test windows_pty_debug -- --ignored --nocapture` while debugging ConPTY or
//! winpty behavior. They are not part of the automated quality gate.

mod support;

#[cfg(windows)]
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
#[cfg(windows)]
use std::io::Read;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
unsafe extern "system" {
    fn AllocConsole() -> i32;
    fn FreeConsole() -> i32;
}

#[cfg(windows)]
struct ConsoleGuard {
    allocated: bool,
}

#[cfg(windows)]
impl ConsoleGuard {
    fn alloc() -> Self {
        let allocated = unsafe { AllocConsole() != 0 };
        Self { allocated }
    }
}

#[cfg(windows)]
impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        if self.allocated {
            let _ = unsafe { FreeConsole() };
        }
    }
}

#[cfg(windows)]
fn raw_portable_pty_smoke(
    cwd: Option<&std::path::Path>,
    env_removals: &[&str],
    env_pairs: &[(&str, &str)],
    take_writer: bool,
    duration_ms: u64,
    respond_to_dsr: bool,
) -> String {
    let pty = match NativePtySystem::default().openpty(PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pty) => pty,
        Err(err) => return format!("openpty failed: {err}"),
    };

    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/D");
    cmd.arg("/C");
    cmd.arg("echo hello & ping -n 5 127.0.0.1 >nul");
    if let Some(cwd) = cwd {
        cmd.cwd(cwd.as_os_str());
    }
    for key in env_removals {
        cmd.env_remove(key);
    }
    for (key, value) in env_pairs {
        cmd.env(key, value);
    }

    let mut child = match pty.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(err) => return format!("spawn_command failed: {err}"),
    };
    let mut writer = if take_writer {
        match pty.master.take_writer() {
            Ok(writer) => Some(writer),
            Err(err) => return format!("take_writer failed: {err}"),
        }
    } else {
        None
    };
    let mut reader = match pty.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(err) => return format!("try_clone_reader failed: {err}"),
    };
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => {
                    if sender.send(buf[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    let mut output = Vec::new();
    let mut responded_to_dsr = false;

    while Instant::now() < deadline {
        while let Ok(chunk) = receiver.try_recv() {
            output.extend_from_slice(&chunk);
        }

        if respond_to_dsr && !responded_to_dsr && output.windows(4).any(|w| w == b"\x1b[6n") {
            if let Some(writer) = writer.as_mut() {
                if let Err(err) = writer.write_all(b"\x1b[1;1R") {
                    return format!("failed to answer DSR probe: {err}");
                }
                if let Err(err) = writer.flush() {
                    return format!("failed to flush DSR probe response: {err}");
                }
                responded_to_dsr = true;
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let text = String::from_utf8_lossy(&output);
                return format!(
                    "exited early with {} (0x{:08X}); output={:?}; responded_to_dsr={responded_to_dsr}",
                    status.exit_code() as i32,
                    status.exit_code(),
                    text
                );
            }
            Ok(None) => {}
            Err(err) => return format!("try_wait failed: {err}"),
        }

        std::thread::sleep(Duration::from_millis(25));
    }

    let _ = child.kill();
    format!(
        "still running after {duration_ms}ms; output={:?}; responded_to_dsr={responded_to_dsr}",
        String::from_utf8_lossy(&output)
    )
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "manual Windows PTY diagnostics"]
async fn windows_pty_debug_report_prints_backend_diagnostics() {
    let tempdir = tempfile::tempdir().unwrap();
    let cmd = vec![
        "cmd.exe".to_string(),
        "/D".to_string(),
        "/C".to_string(),
        "echo hello & ping -n 30 127.0.0.1 >nul".to_string(),
    ];

    let report =
        remote_exec_daemon::exec::session::windows_pty_debug_report(&cmd, tempdir.path()).await;

    let normalized_env = [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("COLORTERM", ""),
        ("PAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("GH_PAGER", "cat"),
        ("CODEX_CI", "1"),
    ];
    let env_removals = ["LANG", "LC_CTYPE", "LC_ALL"];

    println!("{report}");
    println!(
        "raw portable-pty baseline: {}",
        raw_portable_pty_smoke(None, &[], &[], false, 300, false)
    );
    println!(
        "raw portable-pty baseline with writer taken: {}",
        raw_portable_pty_smoke(None, &[], &[], true, 300, false)
    );
    println!(
        "raw portable-pty with cwd: {}",
        raw_portable_pty_smoke(Some(tempdir.path()), &[], &[], false, 300, false)
    );
    println!(
        "raw portable-pty with full env overlay: {}",
        raw_portable_pty_smoke(None, &env_removals, &normalized_env, false, 300, false)
    );
    println!(
        "raw portable-pty with cwd + full env overlay: {}",
        raw_portable_pty_smoke(
            Some(tempdir.path()),
            &env_removals,
            &normalized_env,
            false,
            300,
            false
        )
    );
    println!(
        "raw portable-pty with cwd + full env overlay + writer taken: {}",
        raw_portable_pty_smoke(
            Some(tempdir.path()),
            &env_removals,
            &normalized_env,
            true,
            300,
            false
        )
    );
    println!(
        "raw portable-pty long run with cwd + full env overlay + writer taken: {}",
        raw_portable_pty_smoke(
            Some(tempdir.path()),
            &env_removals,
            &normalized_env,
            true,
            6_000,
            false
        )
    );
    println!(
        "raw portable-pty long run with DSR response: {}",
        raw_portable_pty_smoke(
            Some(tempdir.path()),
            &env_removals,
            &normalized_env,
            true,
            6_000,
            true
        )
    );
    for (key, value) in normalized_env {
        println!(
            "raw portable-pty with only {key}={value:?}: {}",
            raw_portable_pty_smoke(None, &[], &[(key, value)], false, 300, false)
        );
    }
    println!(
        "raw portable-pty with only env removals: {}",
        raw_portable_pty_smoke(None, &env_removals, &[], false, 300, false)
    );
    let _console = ConsoleGuard::alloc();
    println!(
        "raw portable-pty baseline with AllocConsole: {}",
        raw_portable_pty_smoke(None, &[], &[], false, 300, false)
    );

    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let start = fixture
        .rpc::<remote_exec_proto::rpc::ExecStartRequest, remote_exec_proto::rpc::ExecResponse>(
            "/v1/exec/start",
            &remote_exec_proto::rpc::ExecStartRequest {
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
    println!("daemon exec_start response: {start:#?}");
    if let Some(session_id) = start.daemon_session_id() {
        let poll = fixture
            .rpc::<remote_exec_proto::rpc::ExecWriteRequest, remote_exec_proto::rpc::ExecResponse>(
                "/v1/exec/write",
                &remote_exec_proto::rpc::ExecWriteRequest {
                    daemon_session_id: session_id.to_string(),
                    chars: "".to_string(),
                    yield_time_ms: Some(250),
                    max_output_tokens: Some(2_000),
                },
            )
            .await;
        println!("daemon exec_write response: {poll:#?}");
    }
    assert!(report.contains("Windows PTY diagnostics"));
}
