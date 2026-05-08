use std::sync::Arc;
use std::sync::OnceLock;

#[derive(Clone)]
pub struct LocalPortClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl Default for LocalPortClient {
    fn default() -> Self {
        Self::global()
    }
}

impl LocalPortClient {
    pub fn global() -> Self {
        static STATE: OnceLock<Arc<remote_exec_host::HostRuntimeState>> = OnceLock::new();
        let state = STATE
            .get_or_init(|| {
                let config = remote_exec_host::EmbeddedHostConfig {
                    target: "local".to_string(),
                    default_workdir: std::env::current_dir()
                        .unwrap_or_else(|_| std::env::temp_dir()),
                    windows_posix_root: None,
                    sandbox: None,
                    enable_transfer_compression: false,
                    allow_login_shell: false,
                    pty: remote_exec_host::PtyMode::None,
                    default_shell: None,
                    yield_time: remote_exec_host::YieldTimeConfig::default(),
                    port_forward_limits: remote_exec_host::HostPortForwardLimits::default(),
                    experimental_apply_patch_target_encoding_autodetect: false,
                    process_environment: remote_exec_host::ProcessEnvironment::capture_current(),
                };
                Arc::new(
                    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
                        .expect("construct local port runtime"),
                )
            })
            .clone();
        Self { state }
    }

    pub fn state(&self) -> Arc<remote_exec_host::HostRuntimeState> {
        self.state.clone()
    }
}
