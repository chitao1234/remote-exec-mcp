use std::sync::{Arc, OnceLock};

use anyhow::Context;

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
    let config =
        crate::config::LocalTargetConfig::embedded_port_forward_host_config(default_workdir);

    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
}
