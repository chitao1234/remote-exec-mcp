mod capabilities;
mod dispatch;
mod handle;

pub(crate) use dispatch::TargetBackend;
pub(crate) use handle::RemoteTargetHandle;
pub use handle::{CachedDaemonInfo, TargetHandle};

pub(crate) fn ensure_expected_daemon_name(
    target_name: &str,
    expected_daemon_name: Option<&str>,
    actual_daemon_name: &str,
) -> anyhow::Result<()> {
    if let Some(expected_daemon_name) = expected_daemon_name {
        anyhow::ensure!(
            actual_daemon_name == expected_daemon_name,
            "target `{target_name}` resolved to daemon `{actual_daemon_name}` instead of `{expected_daemon_name}`"
        );
    }

    Ok(())
}
