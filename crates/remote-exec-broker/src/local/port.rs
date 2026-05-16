use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS;
use remote_exec_host::{
    HostPortForwardLimits, HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig,
};
use remote_exec_proto::transfer::TransferLimits;

use crate::state::LOCAL_TARGET_NAME;

#[derive(Clone)]
pub struct LocalPortClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl LocalPortClient {
    pub fn global() -> anyhow::Result<Self> {
        static STATE: OnceLock<Result<Arc<remote_exec_host::HostRuntimeState>, String>> =
            OnceLock::new();
        let state = STATE
            .get_or_init(|| {
                build_local_port_runtime()
                    .map(Arc::new)
                    .map_err(|err| format!("{err:#}"))
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(|message| anyhow::anyhow!("constructing local port runtime: {message}"))?;
        Ok(Self { state })
    }

    pub fn state(&self) -> Arc<remote_exec_host::HostRuntimeState> {
        self.state.clone()
    }
}

fn build_local_port_runtime() -> anyhow::Result<remote_exec_host::HostRuntimeState> {
    let default_workdir =
        std::env::current_dir().context("resolving current directory for local port runtime")?;
    let config = local_port_forward_config(default_workdir);

    remote_exec_host::build_runtime_state(config)
}

fn local_port_forward_config(default_workdir: PathBuf) -> HostRuntimeConfig {
    HostRuntimeConfig {
        target: LOCAL_TARGET_NAME.to_string(),
        default_workdir,
        windows_posix_root: None,
        sandbox: None,
        enable_transfer_compression: false,
        transfer_limits: TransferLimits::default(),
        max_open_sessions: DEFAULT_MAX_OPEN_SESSIONS,
        allow_login_shell: false,
        pty: PtyMode::None,
        default_shell: None,
        yield_time: YieldTimeConfig::default(),
        port_forward_limits: HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: ProcessEnvironment::capture_current(),
    }
}
