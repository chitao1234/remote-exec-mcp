#[derive(Clone)]
pub(crate) enum TargetBackend {
    Remote(crate::daemon_client::DaemonClient),
    Local(crate::local_backend::LocalDaemonClient),
}
