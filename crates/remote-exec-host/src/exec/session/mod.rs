mod capability;
mod child;
mod environment;
mod live;
mod spawn;
#[cfg(windows)]
mod windows;

pub use capability::{
    supports_pty, supports_pty_for_mode, supports_pty_with_override, validate_pty_mode,
    windows_pty_backend_override_for_mode,
};
#[cfg(windows)]
pub use spawn::windows_pty_debug_report;
pub use spawn::{spawn, spawn_with_windows_pty_backend_override};

pub use child::PtySession;
pub use live::LiveSession;

pub(crate) use live::OutputWait;

#[cfg(windows)]
use capability::portable_pty_probe;
#[cfg(all(windows, feature = "winpty"))]
pub(super) use child::SessionChild;
#[cfg(all(windows, feature = "winpty"))]
use live::new_live_session;
#[cfg(windows)]
use spawn::spawn_pty;

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::sync::mpsc::unbounded_channel;

    use crate::config::PtyMode;
    use crate::exec::output;

    #[cfg(all(windows, not(feature = "winpty")))]
    use super::capability::validate_pty_mode;
    use super::capability::{supports_pty_for_mode, windows_pty_backend_override_for_mode};
    use super::child::SessionChild;
    use super::live::{LiveSession, new_live_session};

    #[cfg(unix)]
    const TEST_SHELL: &str = "/bin/sh";
    #[cfg(windows)]
    const TEST_SHELL: &str = "cmd.exe";

    #[test]
    fn pty_mode_none_disables_tty_support() {
        assert!(!supports_pty_for_mode(PtyMode::None));
    }

    #[test]
    fn pty_mode_auto_has_no_forced_windows_override() {
        assert_eq!(
            windows_pty_backend_override_for_mode(PtyMode::Auto).unwrap(),
            None
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn forcing_windows_pty_backend_is_rejected_on_non_windows_hosts() {
        assert!(windows_pty_backend_override_for_mode(PtyMode::Conpty).is_err());
        assert!(windows_pty_backend_override_for_mode(PtyMode::Winpty).is_err());
    }

    #[cfg(all(windows, not(feature = "winpty")))]
    #[test]
    fn winpty_mode_is_unavailable_when_the_feature_is_disabled() {
        assert!(!supports_pty_for_mode(PtyMode::Winpty));
        assert!(validate_pty_mode(PtyMode::Winpty).is_err());
    }

    async fn finished_pipe_session(
        receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> LiveSession {
        let mut command = Command::new(TEST_SHELL);
        #[cfg(unix)]
        command.args(["-c", "exit 0"]);
        #[cfg(windows)]
        command.args(["/D", "/C", "exit 0"]);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());

        let child = command.spawn().expect("test child should spawn");
        let mut session = new_live_session(false, SessionChild::Pipe(Box::new(child)), receiver);
        session.exit_code = Some(0);
        session
    }

    #[tokio::test]
    async fn drain_after_exit_waits_for_delayed_pipe_output_until_channel_closes() {
        let (sender, receiver) = unbounded_channel();
        let mut session = finished_pipe_session(receiver).await;

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            sender
                .send("delayed ".to_string())
                .expect("first delayed chunk");
            tokio::time::sleep(Duration::from_millis(50)).await;
            sender
                .send("tail".to_string())
                .expect("second delayed chunk");
        });

        let output = output::drain_after_exit(&mut session)
            .await
            .expect("exit drain should succeed");

        assert_eq!(output, "delayed tail");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_session_termination_kills_pipe_process_group_descendants() {
        use std::time::{Duration, Instant};

        use crate::config::ProcessEnvironment;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let marker = tempdir.path().join("descendant-marker");
        let script = format!(
            "trap 'exit 0' TERM; (trap 'exit 0' TERM; while :; do touch {}; sleep 0.05; done) & echo ready; while :; do sleep 1; done",
            marker.display()
        );
        let cmd = vec![TEST_SHELL.to_string(), "-c".to_string(), script];

        let mut session = super::spawn::spawn(
            &cmd,
            tempdir.path(),
            false,
            &ProcessEnvironment::capture_current(),
        )
        .expect("session should spawn");

        let output = session
            .wait_for_output(Duration::from_secs(2))
            .await
            .expect("wait should succeed");
        match output {
            super::live::OutputWait::Chunk(chunk) => assert!(chunk.contains("ready")),
            _ => panic!("expected ready output"),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "descendant did not create marker");

        session.terminate().await.expect("terminate should succeed");
        let modified_after_terminate = std::fs::metadata(&marker)
            .expect("marker metadata")
            .modified()
            .expect("marker modified time");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let modified_later = std::fs::metadata(&marker)
            .expect("marker metadata after terminate")
            .modified()
            .expect("marker modified time after terminate");

        assert_eq!(
            modified_after_terminate, modified_later,
            "descendant kept running after session termination"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_session_drop_kills_pipe_process_group_descendants() {
        use std::time::{Duration, Instant};

        use crate::config::ProcessEnvironment;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let marker = tempdir.path().join("drop-descendant-marker");
        let script = format!(
            "trap 'exit 0' TERM; (trap 'exit 0' TERM; i=0; while [ \"$i\" -lt 100 ]; do touch {}; i=$((i + 1)); sleep 0.05; done) & echo ready; while :; do sleep 1; done",
            marker.display()
        );
        let cmd = vec![TEST_SHELL.to_string(), "-c".to_string(), script];

        let mut session = super::spawn::spawn(
            &cmd,
            tempdir.path(),
            false,
            &ProcessEnvironment::capture_current(),
        )
        .expect("session should spawn");

        let output = session
            .wait_for_output(Duration::from_secs(2))
            .await
            .expect("wait should succeed");
        match output {
            super::live::OutputWait::Chunk(chunk) => assert!(chunk.contains("ready")),
            _ => panic!("expected ready output"),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "descendant did not create marker");

        drop(session);
        let modified_after_drop = std::fs::metadata(&marker)
            .expect("marker metadata")
            .modified()
            .expect("marker modified time");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let modified_later = std::fs::metadata(&marker)
            .expect("marker metadata after drop")
            .modified()
            .expect("marker modified time after drop");

        assert_eq!(
            modified_after_drop, modified_later,
            "descendant kept running after LiveSession drop"
        );
    }
}
