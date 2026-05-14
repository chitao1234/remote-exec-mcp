use std::sync::Arc;

use remote_exec_proto::rpc::{PortForwardProtocolVersion, TargetInfoResponse};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{HostRuntimeConfig, WindowsPtyBackendOverride, sandbox::CompiledFilesystemSandbox};

#[derive(Clone, Default)]
pub struct BackgroundTasks {
    tasks: Arc<Mutex<JoinSet<()>>>,
}

impl BackgroundTasks {
    pub async fn spawn<F>(&self, name: &'static str, task: F)
    where
        F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.tasks.lock().await.spawn(async move {
            if let Err(err) = task.await {
                tracing::warn!(task = name, ?err, "background task failed");
            }
        });
    }

    pub async fn join_all(&self) {
        loop {
            let mut tasks = {
                let mut tasks = self.tasks.lock().await;
                if tasks.is_empty() {
                    return;
                }
                std::mem::take(&mut *tasks)
            };
            while let Some(result) = tasks.join_next().await {
                if let Err(err) = result {
                    tracing::warn!(?err, "background task join failed");
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct HostRuntimeState {
    pub config: Arc<HostRuntimeConfig>,
    pub default_shell: String,
    pub sandbox: Option<CompiledFilesystemSandbox>,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub daemon_instance_id: String,
    pub shutdown: CancellationToken,
    pub sessions: crate::exec::store::SessionStore,
    pub port_forward_sessions: crate::port_forward::TunnelSessionStore,
    pub port_forward_limiter: Arc<crate::port_forward::PortForwardLimiter>,
    pub background_tasks: BackgroundTasks,
}

pub fn build_runtime_state(mut config: HostRuntimeConfig) -> anyhow::Result<HostRuntimeState> {
    config.normalize_paths();
    config.validate()?;
    let sandbox = config
        .sandbox
        .as_ref()
        .map(crate::sandbox::compile_filesystem_sandbox)
        .transpose()?;
    let default_shell = crate::exec::shell::resolve_default_shell(
        config.default_shell.as_deref(),
        &config.process_environment,
        config.windows_posix_root.as_deref(),
    )?;
    crate::exec::session::validate_pty_mode(config.pty)?;
    let supports_pty = crate::exec::session::supports_pty_for_mode(config.pty);
    let supports_transfer_compression = config.enable_transfer_compression;
    let windows_pty_backend_override =
        crate::exec::session::windows_pty_backend_override_for_mode(config.pty)?;

    let port_forward_limits = config.port_forward_limits;
    let max_open_sessions = config.max_open_sessions;

    Ok(HostRuntimeState {
        config: Arc::new(config),
        default_shell,
        sandbox,
        supports_pty,
        supports_transfer_compression,
        windows_pty_backend_override,
        daemon_instance_id: crate::ids::new_instance_id(),
        shutdown: CancellationToken::new(),
        sessions: crate::exec::store::SessionStore::new(max_open_sessions),
        port_forward_sessions: crate::port_forward::TunnelSessionStore::default(),
        port_forward_limiter: Arc::new(crate::port_forward::PortForwardLimiter::new(
            port_forward_limits,
        )),
        background_tasks: BackgroundTasks::default(),
    })
}

pub fn target_info_response(state: &HostRuntimeState, daemon_version: &str) -> TargetInfoResponse {
    TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: daemon_version.to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: state.supports_pty,
        supports_image_read: true,
        supports_transfer_compression: state.supports_transfer_compression,
        supports_port_forward: true,
        port_forward_protocol_version: Some(PortForwardProtocolVersion::v4()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::oneshot;

    use super::BackgroundTasks;

    #[tokio::test(flavor = "current_thread")]
    async fn join_all_does_not_block_spawn_while_waiting_for_tracked_tasks() {
        let tasks = BackgroundTasks::default();
        let (allow_parent_tx, allow_parent_rx) = oneshot::channel();

        tasks
            .spawn("parent", async move {
                let _ = allow_parent_rx.await;
                Ok(())
            })
            .await;

        let join_handle = tokio::spawn({
            let tasks = tasks.clone();
            async move { tasks.join_all().await }
        });
        tokio::task::yield_now().await;

        tokio::time::timeout(Duration::from_millis(200), async {
            tasks
                .spawn("child", async move { Ok(()) })
                .await;
        })
        .await
        .expect("join_all should not hold the task set mutex while awaiting joins");

        allow_parent_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), join_handle)
            .await
            .expect("join_all should finish after tracked tasks complete")
            .unwrap();
    }
}
