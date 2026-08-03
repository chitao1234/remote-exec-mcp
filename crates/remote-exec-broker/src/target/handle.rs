use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use remote_exec_proto::path::{PathPolicy, linux_path_policy, windows_path_policy};
use remote_exec_proto::rpc::{
    DaemonIdentity, ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest,
    FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse,
    HealthCheckResponse, ImageReadRequest, ImageReadResponse, PatchApplyRequest,
    PatchApplyResponse, TargetCapabilities, TargetInfoResponse, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};
use tokio::sync::Mutex;

use crate::daemon_client::{DaemonClientError, TransferExportResponse, TransferExportStream};

use super::{TargetBackend, ensure_expected_daemon_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub target: String,
    pub daemon_instance_id: String,
    pub identity: DaemonIdentity,
    pub capabilities: TargetCapabilities,
    pub supports_transfer_compression: bool,
}

impl CachedDaemonInfo {
    pub(crate) fn platform_is_windows(&self) -> bool {
        self.identity.platform.eq_ignore_ascii_case("windows")
    }

    pub(crate) fn path_policy(&self) -> PathPolicy {
        if self.platform_is_windows() {
            windows_path_policy()
        } else {
            linux_path_policy()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTargetHealth {
    pub status: CachedTargetHealthStatus,
    pub daemon_version: Option<String>,
    pub daemon_instance_id: Option<String>,
    pub last_checked_at: SystemTime,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachedTargetHealthStatus {
    Healthy,
    MaybeUnhealthy,
    Unhealthy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CachedTargetStatus {
    pub(crate) healthy: bool,
    pub(crate) health_status: Option<CachedTargetHealthStatus>,
    pub(crate) daemon_info: Option<CachedDaemonInfo>,
}

impl CachedTargetHealth {
    fn healthy(response: &HealthCheckResponse) -> Self {
        Self {
            status: CachedTargetHealthStatus::Healthy,
            daemon_version: Some(response.daemon_version.clone()),
            daemon_instance_id: Some(response.daemon_instance_id.clone()),
            last_checked_at: SystemTime::now(),
            last_error: None,
        }
    }

    fn record_failure(previous: Option<&Self>, error: String) -> Self {
        let status = match previous.map(|health| health.status) {
            Some(CachedTargetHealthStatus::MaybeUnhealthy)
            | Some(CachedTargetHealthStatus::Unhealthy) => CachedTargetHealthStatus::Unhealthy,
            Some(CachedTargetHealthStatus::Healthy) | None => {
                CachedTargetHealthStatus::MaybeUnhealthy
            }
        };
        Self {
            status,
            daemon_version: previous.and_then(|health| health.daemon_version.clone()),
            daemon_instance_id: previous.and_then(|health| health.daemon_instance_id.clone()),
            last_checked_at: SystemTime::now(),
            last_error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TargetRuntimeSnapshot {
    daemon_info: Option<CachedDaemonInfo>,
    health: Option<CachedTargetHealth>,
}

impl TargetRuntimeSnapshot {
    fn verified(info: &TargetInfoResponse) -> Self {
        Self {
            daemon_info: Some(TargetHandle::cache_from_target_info(info)),
            health: Some(CachedTargetHealth::from_target_info(info)),
        }
    }

    fn available(&self) -> bool {
        self.health_ok() && self.daemon_info.is_some()
    }

    fn health_ok(&self) -> bool {
        self.health
            .as_ref()
            .is_some_and(|health| health.status != CachedTargetHealthStatus::Unhealthy)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetAvailabilityTransition {
    AvailableToUnavailable,
    UnavailableToAvailable,
}

impl TargetAvailabilityTransition {
    fn from_snapshots(
        previous: &TargetRuntimeSnapshot,
        current: &TargetRuntimeSnapshot,
    ) -> Option<Self> {
        match (previous.available(), current.available()) {
            (true, false) => Some(Self::AvailableToUnavailable),
            (false, true) => Some(Self::UnavailableToAvailable),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct TargetRuntimeState {
    snapshot: TargetRuntimeSnapshot,
}

impl TargetRuntimeState {
    fn new(daemon_info: Option<CachedDaemonInfo>, health: Option<CachedTargetHealth>) -> Self {
        Self {
            snapshot: TargetRuntimeSnapshot {
                daemon_info,
                health,
            },
        }
    }

    fn snapshot(&self) -> TargetRuntimeSnapshot {
        self.snapshot.clone()
    }

    fn daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.snapshot.daemon_info.clone()
    }

    fn record_health_failure(&mut self, error: &DaemonClientError) {
        self.record_health_failure_message(error.to_string());
    }

    fn record_health_failure_message(&mut self, error: String) {
        self.snapshot.health = Some(CachedTargetHealth::record_failure(
            self.snapshot.health.as_ref(),
            error,
        ));
    }

    fn set_verified_target_info(&mut self, info: &TargetInfoResponse) {
        self.snapshot = TargetRuntimeSnapshot::verified(info);
    }

    fn update_healthy(&mut self, health: &HealthCheckResponse) -> Option<String> {
        let previous_daemon_instance_id = self
            .snapshot
            .daemon_info
            .as_ref()
            .map(|info| info.daemon_instance_id.clone());
        let instance_changed = self
            .snapshot
            .daemon_info
            .as_ref()
            .map(|info| info.daemon_instance_id != health.daemon_instance_id)
            .unwrap_or(false);

        self.snapshot.health = Some(CachedTargetHealth::healthy(health));
        if instance_changed {
            self.snapshot.daemon_info = None;
        }

        instance_changed
            .then_some(previous_daemon_instance_id)
            .flatten()
    }
}

impl CachedTargetHealth {
    fn from_target_info(info: &TargetInfoResponse) -> Self {
        Self {
            status: CachedTargetHealthStatus::Healthy,
            daemon_version: Some(info.identity.daemon_version.clone()),
            daemon_instance_id: Some(info.daemon_instance_id.clone()),
            last_checked_at: SystemTime::now(),
            last_error: None,
        }
    }
}

#[derive(Clone)]
pub struct TargetHandle {
    pub(super) backend: TargetBackend,
    expected_daemon_name: Option<String>,
    runtime: Arc<Mutex<TargetRuntimeState>>,
    probe_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Copy)]
pub(crate) struct RemoteTargetHandle<'a> {
    handle: &'a TargetHandle,
    client: &'a crate::daemon_client::DaemonClient,
}

impl TargetHandle {
    fn new(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        cached_daemon_info: Option<CachedDaemonInfo>,
        cached_health: Option<CachedTargetHealth>,
    ) -> Self {
        Self {
            backend,
            expected_daemon_name,
            runtime: Arc::new(Mutex::new(TargetRuntimeState::new(
                cached_daemon_info,
                cached_health,
            ))),
            probe_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn verified(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        info: &TargetInfoResponse,
    ) -> Self {
        Self::new(
            backend,
            expected_daemon_name,
            Some(Self::cache_from_target_info(info)),
            Some(CachedTargetHealth::from_target_info(info)),
        )
    }

    pub(crate) fn unavailable(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
    ) -> Self {
        Self::new(backend, expected_daemon_name, None, None)
    }

    fn cache_from_target_info(info: &TargetInfoResponse) -> CachedDaemonInfo {
        CachedDaemonInfo {
            target: info.target.clone(),
            daemon_instance_id: info.daemon_instance_id.clone(),
            identity: info.identity.clone(),
            capabilities: info.capabilities.clone(),
            supports_transfer_compression: info.supports_transfer_compression,
        }
    }

    async fn runtime_snapshot(&self) -> TargetRuntimeSnapshot {
        self.runtime.lock().await.snapshot()
    }

    pub(crate) async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.runtime_snapshot().await.daemon_info
    }

    pub(crate) async fn cached_health(&self) -> Option<CachedTargetHealth> {
        self.runtime_snapshot().await.health
    }

    pub(crate) async fn cached_status(&self) -> CachedTargetStatus {
        let snapshot = self.runtime_snapshot().await;
        let available = snapshot.available();
        let daemon_info = available.then_some(snapshot.daemon_info).flatten();
        CachedTargetStatus {
            healthy: available,
            health_status: snapshot.health.map(|health| health.status),
            daemon_info,
        }
    }

    pub(crate) async fn needs_status_recheck(&self) -> bool {
        let snapshot = self.runtime_snapshot().await;
        !snapshot.available()
    }

    pub(crate) fn health_probe_timeout(&self) -> Option<Duration> {
        self.backend.health_probe_timeout()
    }

    pub(crate) async fn recover_connection_after_timeout(&self) -> Option<String> {
        self.backend.recover_connection_after_timeout().await
    }

    pub(crate) async fn cached_daemon_info_after_verification(
        &self,
        name: &str,
    ) -> anyhow::Result<CachedDaemonInfo> {
        self.ensure_daemon_info_cached(name).await?;
        self.cached_daemon_info()
            .await
            .context("target info missing after identity verification")
    }

    pub(crate) async fn health(&self) -> Result<HealthCheckResponse, DaemonClientError> {
        self.backend.health().await
    }

    pub(crate) async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.backend.target_info().await
    }

    pub(crate) async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.backend.exec_start(req).await
    }

    pub(crate) async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.backend.exec_write(req).await
    }

    pub(crate) async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        self.backend.patch_apply(req).await
    }

    pub(crate) async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        self.backend.image_read(req).await
    }

    pub(crate) async fn file_read(
        &self,
        req: &FileReadRequest,
    ) -> Result<FileReadResponse, DaemonClientError> {
        self.backend.file_read(req).await
    }

    pub(crate) async fn file_write(
        &self,
        req: &FileWriteRequest,
    ) -> Result<FileWriteResponse, DaemonClientError> {
        self.backend.file_write(req).await
    }

    pub(crate) async fn file_edit(
        &self,
        req: &FileEditRequest,
    ) -> Result<FileEditResponse, DaemonClientError> {
        self.backend.file_edit(req).await
    }

    pub(crate) fn as_remote(&self) -> Option<RemoteTargetHandle<'_>> {
        self.backend
            .remote_client()
            .map(|client| RemoteTargetHandle {
                handle: self,
                client,
            })
    }

    pub(crate) async fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> Result<crate::port_forward::PortTunnel, DaemonClientError> {
        self.backend.port_tunnel(max_queued_bytes).await
    }

    pub(crate) async fn ensure_daemon_info_cached(&self, name: &str) -> anyhow::Result<()> {
        if self.runtime.lock().await.daemon_info().is_some() {
            return Ok(());
        }

        let _probe_guard = self.probe_lock.lock().await;
        if self.runtime.lock().await.daemon_info().is_some() {
            return Ok(());
        }

        let previous_snapshot = self.runtime_snapshot().await;
        let info = match self.target_info().await {
            Ok(info) => info,
            Err(DaemonClientError::Transport(err)) => {
                tracing::warn!(target = %name, ?err, "target identity verification failed");
                return Err(DaemonClientError::Transport(err).into());
            }
            Err(err) => return Err(err.into()),
        };
        ensure_expected_daemon_name(name, self.expected_daemon_name.as_deref(), &info.target)?;

        self.runtime.lock().await.set_verified_target_info(&info);
        self.log_availability_transition_since(name, &previous_snapshot, None)
            .await;
        Ok(())
    }

    pub(crate) async fn refresh_health_and_cache(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let _probe_guard = self.probe_lock.lock().await;
        let previous_snapshot = self.runtime_snapshot().await;
        match self.health().await {
            Ok(health) => {
                let (previous_daemon_instance_id, needs_target_info) = {
                    let mut runtime = self.runtime.lock().await;
                    let previous_daemon_instance_id = runtime.update_healthy(&health);
                    (
                        previous_daemon_instance_id,
                        runtime.snapshot.daemon_info.is_none(),
                    )
                };

                if needs_target_info {
                    let info = match self.target_info().await {
                        Ok(info) => info,
                        Err(err) => {
                            let error = err.to_string();
                            self.log_availability_transition_since(
                                name,
                                &previous_snapshot,
                                Some(error.as_str()),
                            )
                            .await;
                            return Err(err.into());
                        }
                    };
                    if let Err(err) = ensure_expected_daemon_name(
                        name,
                        self.expected_daemon_name.as_deref(),
                        &info.target,
                    ) {
                        let error = err.to_string();
                        self.log_availability_transition_since(
                            name,
                            &previous_snapshot,
                            Some(error.as_str()),
                        )
                        .await;
                        return Err(err);
                    }
                    self.runtime.lock().await.set_verified_target_info(&info);
                }

                self.log_availability_transition_since(name, &previous_snapshot, None)
                    .await;
                Ok(previous_daemon_instance_id)
            }
            Err(err) => {
                self.runtime.lock().await.record_health_failure(&err);
                let error = err.to_string();
                self.log_availability_transition_since(
                    name,
                    &previous_snapshot,
                    Some(error.as_str()),
                )
                .await;
                if let DaemonClientError::Rpc { .. } | DaemonClientError::Decode(_) = &err {
                    tracing::debug!(target = %name, error = %err, "target health refresh failed");
                } else {
                    tracing::debug!(target = %name, error = %err, "target unreachable during health refresh");
                }
                Err(err.into())
            }
        }
    }

    pub(crate) async fn mark_health_probe_timed_out(&self, name: &str, error: String) {
        let (previous_snapshot, current_snapshot) = {
            let mut runtime = self.runtime.lock().await;
            let previous_snapshot = runtime.snapshot();
            runtime.record_health_failure_message(error.clone());
            let current_snapshot = runtime.snapshot();
            (previous_snapshot, current_snapshot)
        };

        if !log_target_availability_transition(
            name,
            &previous_snapshot,
            &current_snapshot,
            Some(error.as_str()),
        ) {
            tracing::debug!(
                target = %name,
                error = %error,
                "target health probe timed out"
            );
        }
    }

    async fn log_availability_transition_since(
        &self,
        name: &str,
        previous_snapshot: &TargetRuntimeSnapshot,
        error: Option<&str>,
    ) -> bool {
        let current_snapshot = self.runtime_snapshot().await;
        log_target_availability_transition(name, previous_snapshot, &current_snapshot, error)
    }

    pub(crate) async fn invalidate_cached_daemon_info(&self, name: &str) {
        let (previous_snapshot, current_snapshot) = {
            let mut runtime = self.runtime.lock().await;
            let previous_snapshot = runtime.snapshot();
            runtime.snapshot.daemon_info = None;
            let current_snapshot = runtime.snapshot();
            (previous_snapshot, current_snapshot)
        };

        if !log_target_availability_transition(name, &previous_snapshot, &current_snapshot, None) {
            tracing::info!(
                target = %name,
                "cleared cached daemon identity and metadata"
            );
        }
    }
}

fn log_target_availability_transition(
    name: &str,
    previous: &TargetRuntimeSnapshot,
    current: &TargetRuntimeSnapshot,
    error: Option<&str>,
) -> bool {
    match TargetAvailabilityTransition::from_snapshots(previous, current) {
        Some(TargetAvailabilityTransition::AvailableToUnavailable) => {
            log_target_became_unavailable(name, previous, error);
            true
        }
        Some(TargetAvailabilityTransition::UnavailableToAvailable) => {
            log_target_became_available(name, current);
            true
        }
        None => false,
    }
}

fn log_target_became_available(name: &str, current: &TargetRuntimeSnapshot) {
    let info = current
        .daemon_info
        .as_ref()
        .expect("available target should have cached daemon info");
    tracing::info!(
        target = %name,
        daemon_name = %info.target,
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.identity.platform,
        arch = %info.identity.arch,
        hostname = %info.identity.hostname,
        supports_pty = info.capabilities.supports_pty,
        supports_transfer_compression = info.supports_transfer_compression,
        "target transitioned from unavailable to available"
    );
}

fn log_target_became_unavailable(
    name: &str,
    previous: &TargetRuntimeSnapshot,
    error: Option<&str>,
) {
    let info = previous
        .daemon_info
        .as_ref()
        .expect("previously available target should have cached daemon info");
    if let Some(error) = error {
        tracing::info!(
            target = %name,
            daemon_name = %info.target,
            daemon_instance_id = %info.daemon_instance_id,
            error = %error,
            "target transitioned from available to unavailable"
        );
    } else {
        tracing::info!(
            target = %name,
            daemon_name = %info.target,
            daemon_instance_id = %info.daemon_instance_id,
            "target transitioned from available to unavailable"
        );
    }
}

impl RemoteTargetHandle<'_> {
    pub(crate) async fn cached_daemon_info_after_verification(
        &self,
        name: &str,
    ) -> anyhow::Result<CachedDaemonInfo> {
        self.handle
            .cached_daemon_info_after_verification(name)
            .await
    }

    pub(crate) async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.handle.target_info().await
    }

    #[allow(dead_code)]
    pub(crate) async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferExportResponse, DaemonClientError> {
        self.client.transfer_export_to_file(req, archive_path).await
    }

    pub(crate) async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        self.client.transfer_export_stream(req).await
    }

    pub(crate) async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.client.transfer_path_info(req).await
    }

    #[allow(dead_code)]
    pub(crate) async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        self.client
            .transfer_import_from_file(archive_path, req)
            .await
    }

    pub(crate) async fn transfer_import_from_archive_stream<S, E>(
        &self,
        req: &TransferImportRequest,
        stream: S,
    ) -> Result<TransferImportResponse, DaemonClientError>
    where
        S: futures_util::TryStream<Ok = bytes::Bytes, Error = E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.client
            .transfer_import_from_archive_stream(req, stream)
            .await
    }
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::{
        DaemonIdentity, FileToolProtocolVersion, HealthCheckResponse, HealthStatus,
        PortForwardProtocolVersion, TargetCapabilities, TargetInfoResponse,
        TransferStreamProtocolVersion,
    };

    use super::{TargetAvailabilityTransition, TargetRuntimeSnapshot, TargetRuntimeState};

    #[test]
    fn healthy_refresh_for_new_instance_clears_stale_daemon_info_in_same_snapshot() {
        let old_info = target_info("daemon-old", "1.0.0");
        let mut state = TargetRuntimeState::new(
            Some(super::TargetHandle::cache_from_target_info(&old_info)),
            Some(super::CachedTargetHealth::from_target_info(&old_info)),
        );

        let previous = state.update_healthy(&health("daemon-new", "1.0.1"));
        let snapshot = state.snapshot();

        assert_eq!(previous.as_deref(), Some("daemon-old"));
        assert_eq!(
            snapshot
                .health
                .as_ref()
                .and_then(|health| health.daemon_instance_id.as_deref()),
            Some("daemon-new")
        );
        assert_eq!(snapshot.daemon_info, None);
    }

    #[test]
    fn verified_target_info_replaces_health_and_metadata_as_one_snapshot() {
        let old_info = target_info("daemon-old", "1.0.0");
        let mut state = TargetRuntimeState::new(
            Some(super::TargetHandle::cache_from_target_info(&old_info)),
            Some(super::CachedTargetHealth::from_target_info(&old_info)),
        );

        let new_info = target_info("daemon-new", "1.0.1");
        state.set_verified_target_info(&new_info);
        let snapshot = state.snapshot();

        assert_eq!(
            snapshot
                .daemon_info
                .as_ref()
                .map(|info| info.daemon_instance_id.as_str()),
            Some("daemon-new")
        );
        assert_eq!(
            snapshot
                .health
                .as_ref()
                .and_then(|health| health.daemon_instance_id.as_deref()),
            Some("daemon-new")
        );
        assert_eq!(
            snapshot
                .health
                .as_ref()
                .and_then(|health| health.daemon_version.as_deref()),
            Some("1.0.1")
        );
    }

    #[test]
    fn availability_transition_detects_available_to_unavailable() {
        let info = target_info("daemon-old", "1.0.0");
        let previous = TargetRuntimeSnapshot::verified(&info);
        let current = TargetRuntimeSnapshot {
            daemon_info: Some(super::TargetHandle::cache_from_target_info(&info)),
            health: None,
        };

        assert_eq!(
            TargetAvailabilityTransition::from_snapshots(&previous, &current),
            Some(TargetAvailabilityTransition::AvailableToUnavailable)
        );
    }

    #[test]
    fn availability_transition_detects_unavailable_to_available() {
        let previous = TargetRuntimeSnapshot::default();
        let current = TargetRuntimeSnapshot::verified(&target_info("daemon-new", "1.0.1"));

        assert_eq!(
            TargetAvailabilityTransition::from_snapshots(&previous, &current),
            Some(TargetAvailabilityTransition::UnavailableToAvailable)
        );
    }

    #[test]
    fn availability_transition_ignores_unchanged_availability() {
        let info = target_info("daemon-old", "1.0.0");
        let available = TargetRuntimeSnapshot::verified(&info);
        let unavailable = TargetRuntimeSnapshot::default();

        assert_eq!(
            TargetAvailabilityTransition::from_snapshots(&available, &available),
            None
        );
        assert_eq!(
            TargetAvailabilityTransition::from_snapshots(&unavailable, &unavailable),
            None
        );
    }

    #[test]
    fn first_health_failure_marks_verified_target_maybe_unhealthy() {
        let info = target_info("daemon-old", "1.0.0");
        let mut state = TargetRuntimeState::new(
            Some(super::TargetHandle::cache_from_target_info(&info)),
            Some(super::CachedTargetHealth::from_target_info(&info)),
        );

        state.record_health_failure_message("temporary failure".to_string());
        let snapshot = state.snapshot();

        assert!(snapshot.available());
        assert_eq!(
            snapshot.health.as_ref().map(|health| health.status),
            Some(super::CachedTargetHealthStatus::MaybeUnhealthy)
        );
    }

    #[test]
    fn second_consecutive_health_failure_marks_target_unhealthy() {
        let info = target_info("daemon-old", "1.0.0");
        let mut state = TargetRuntimeState::new(
            Some(super::TargetHandle::cache_from_target_info(&info)),
            Some(super::CachedTargetHealth::from_target_info(&info)),
        );

        state.record_health_failure_message("first failure".to_string());
        state.record_health_failure_message("second failure".to_string());
        let snapshot = state.snapshot();

        assert!(!snapshot.available());
        assert_eq!(
            snapshot.health.as_ref().map(|health| health.status),
            Some(super::CachedTargetHealthStatus::Unhealthy)
        );
    }

    #[test]
    fn successful_health_check_clears_maybe_unhealthy_state() {
        let info = target_info("daemon-old", "1.0.0");
        let mut state = TargetRuntimeState::new(
            Some(super::TargetHandle::cache_from_target_info(&info)),
            Some(super::CachedTargetHealth::from_target_info(&info)),
        );

        state.record_health_failure_message("temporary failure".to_string());
        state.update_healthy(&health("daemon-old", "1.0.0"));
        let snapshot = state.snapshot();

        assert!(snapshot.available());
        assert_eq!(
            snapshot.health.as_ref().map(|health| health.status),
            Some(super::CachedTargetHealthStatus::Healthy)
        );
    }

    fn health(daemon_instance_id: &str, daemon_version: &str) -> HealthCheckResponse {
        HealthCheckResponse {
            status: HealthStatus::Ok,
            daemon_version: daemon_version.to_string(),
            daemon_instance_id: daemon_instance_id.to_string(),
        }
    }

    fn target_info(daemon_instance_id: &str, daemon_version: &str) -> TargetInfoResponse {
        TargetInfoResponse {
            target: "target-a".to_string(),
            daemon_instance_id: daemon_instance_id.to_string(),
            identity: DaemonIdentity {
                daemon_version: daemon_version.to_string(),
                hostname: "host-a".to_string(),
                platform: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
            capabilities: TargetCapabilities {
                supports_pty: true,
                supports_port_forward: true,
                port_forward_protocol_version: Some(PortForwardProtocolVersion::v4()),
                transfer_stream_protocol_version: Some(TransferStreamProtocolVersion::v2()),
                file_tool_protocol_version: Some(FileToolProtocolVersion::v1()),
            },
            supports_image_read: true,
            supports_transfer_compression: true,
        }
    }
}
