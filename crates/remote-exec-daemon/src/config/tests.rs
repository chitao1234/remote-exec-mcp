use std::path::Path;

use super::{
    DaemonConfig, DaemonTransport, ValidatedDaemonConfig, YieldTimeConfig, YieldTimeOperation,
};

fn neutral_toml_path(path: &Path) -> toml::Value {
    toml::Value::String(path.display().to_string())
}

fn neutral_workdir(dir: &tempfile::TempDir) -> toml::Value {
    neutral_toml_path(dir.path())
}

fn http_config(workdir: toml::Value, extra: &str) -> String {
    format!(
        r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {workdir}
transport = "http"
{extra}"#
    )
}

async fn load_config(
    dir: &tempfile::TempDir,
    text: impl AsRef<str>,
) -> anyhow::Result<ValidatedDaemonConfig> {
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(&config_path, text.as_ref()).await?;
    DaemonConfig::load(&config_path).await
}

#[tokio::test]
async fn load_accepts_http_transport_without_tls_block() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(&dir, http_config(neutral_workdir(&dir), ""))
        .await
        .unwrap();
    assert!(matches!(config.transport, DaemonTransport::Http));
    assert!(config.http_auth.is_none());
    assert!(config.tls.is_none());
    assert_eq!(config.yield_time, YieldTimeConfig::default());
    assert_eq!(
        config.max_open_sessions,
        remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS
    );
    assert!(!config.experimental_apply_patch_target_encoding_autodetect);
}

#[tokio::test]
async fn load_accepts_max_open_sessions_override() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        http_config(neutral_workdir(&dir), "max_open_sessions = 7\n"),
    )
    .await
    .unwrap();
    assert_eq!(config.max_open_sessions, 7);
    assert_eq!(
        remote_exec_host::HostRuntimeConfig::from(&config).max_open_sessions,
        7
    );
}

#[tokio::test]
async fn load_rejects_zero_max_open_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_config(
        &dir,
        http_config(neutral_workdir(&dir), "max_open_sessions = 0\n"),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("max_open_sessions must be greater than zero"),
        "unexpected error: {err}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn load_accepts_windows_posix_root() {
    let dir = tempfile::tempdir().unwrap();
    let synthetic_root = dir.path().join("msys64");
    let config = load_config(
        &dir,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
windows_posix_root = {}
transport = "http"
"#,
            neutral_workdir(&dir),
            neutral_toml_path(&synthetic_root)
        ),
    )
    .await
    .unwrap();
    assert_eq!(config.windows_posix_root, Some(synthetic_root));
}

#[cfg(windows)]
#[tokio::test]
async fn load_normalizes_default_workdir_through_windows_posix_root() {
    let dir = tempfile::tempdir().unwrap();
    let synthetic_root = dir.path().join("msys64");
    let posix_workdir_name = "tmp";
    let posix_workdir = format!("/{posix_workdir_name}");
    std::fs::create_dir_all(synthetic_root.join(posix_workdir_name)).unwrap();
    let config = load_config(
        &dir,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = "{posix_workdir}"
windows_posix_root = {}
transport = "http"
"#,
            neutral_toml_path(&synthetic_root)
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        config.default_workdir,
        synthetic_root.join(posix_workdir_name)
    );
}

#[tokio::test]
async fn load_rejects_default_tls_transport_without_tls_block() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    if cfg!(feature = "tls") {
        assert!(
            err.to_string()
                .contains("tls config is required when transport = \"tls\""),
            "unexpected error: {err}"
        );
    } else {
        assert!(
            err.to_string()
                .contains(crate::tls::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}"
        );
    }
}

#[tokio::test]
async fn load_rejects_missing_default_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let missing_workdir = dir.path().join("missing-workdir");
    let err = load_config(&dir, http_config(neutral_toml_path(&missing_workdir), ""))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("default_workdir") && err.to_string().contains("does not exist"),
        "unexpected error: {err}"
    );
}

#[test]
fn yield_time_defaults_preserve_existing_behavior() {
    let config = YieldTimeConfig::default();

    assert_eq!(
        config.resolve_ms(YieldTimeOperation::ExecCommand, None),
        10_000
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::ExecCommand, Some(1)),
        250
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::WriteStdinPoll, None),
        5_000
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::WriteStdinPoll, Some(1)),
        5_000
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::WriteStdinPoll, Some(400_000)),
        300_000
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::WriteStdinInput, None),
        250
    );
    assert_eq!(
        config.resolve_ms(YieldTimeOperation::WriteStdinInput, Some(100_000)),
        30_000
    );
}

#[tokio::test]
async fn load_merges_partial_yield_time_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[yield_time.exec_command]
max_ms = 60000

[yield_time.write_stdin_poll]
default_ms = 12000
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let config = DaemonConfig::load(&config_path).await.unwrap();
    assert_eq!(config.yield_time.exec_command.default_ms, 10_000);
    assert_eq!(config.yield_time.exec_command.min_ms, 250);
    assert_eq!(config.yield_time.exec_command.max_ms, 60_000);
    assert_eq!(config.yield_time.write_stdin_poll.default_ms, 12_000);
    assert_eq!(config.yield_time.write_stdin_poll.min_ms, 5_000);
    assert_eq!(config.yield_time.write_stdin_poll.max_ms, 300_000);
    assert_eq!(config.yield_time.write_stdin_input.default_ms, 250);
}

#[tokio::test]
async fn load_accepts_port_forward_connect_timeout_override() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[port_forward_limits]
connect_timeout_ms = 7000
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let config = DaemonConfig::load(&config_path).await.unwrap();
    assert_eq!(config.port_forward_limits.connect_timeout_ms, 7_000);
}

#[tokio::test]
async fn load_rejects_zero_port_forward_connect_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[port_forward_limits]
connect_timeout_ms = 0
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("port_forward_limits.connect_timeout_ms must be greater than zero"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_rejects_invalid_transfer_limit_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[transfer_limits]
max_archive_bytes = 8
max_entry_bytes = 16
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string().contains(
            "transfer_limits.max_entry_bytes must be less than or equal to transfer_limits.max_archive_bytes"
        ),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_rejects_invalid_yield_time_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[yield_time.exec_command]
default_ms = 100
min_ms = 200
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("yield_time.exec_command.default_ms must be between"),
        "unexpected error: {err}"
    );
}

#[cfg(feature = "tls")]
#[tokio::test]
async fn load_accepts_tls_transport_with_pinned_client_cert() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    let daemon_cert = dir.path().join("daemon.pem");
    let daemon_key = dir.path().join("daemon.key");
    let ca_cert = dir.path().join("ca.pem");
    let broker_cert = dir.path().join("broker.pem");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
pinned_client_cert_pem = {}
"#,
            neutral_workdir(&dir),
            neutral_toml_path(&daemon_cert),
            neutral_toml_path(&daemon_key),
            neutral_toml_path(&ca_cert),
            neutral_toml_path(&broker_cert)
        ),
    )
    .await
    .unwrap();

    let config = DaemonConfig::load(&config_path).await.unwrap();
    assert_eq!(
        config
            .tls
            .as_ref()
            .and_then(|tls| tls.pinned_client_cert_pem.as_ref()),
        Some(&broker_cert)
    );
}

#[tokio::test]
async fn load_accepts_experimental_apply_patch_target_encoding_autodetect() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
transport = "http"
experimental_apply_patch_target_encoding_autodetect = true
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let config = DaemonConfig::load(&config_path).await.unwrap();
    assert!(config.experimental_apply_patch_target_encoding_autodetect);
}

#[tokio::test]
async fn load_accepts_http_bearer_auth() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[http_auth]
bearer_token = "shared-secret"
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let config = DaemonConfig::load(&config_path).await.unwrap();
    assert_eq!(
        config
            .http_auth
            .as_ref()
            .map(|auth| auth.bearer_token.as_str()),
        Some("shared-secret")
    );
    assert_eq!(
        config
            .http_auth
            .as_ref()
            .map(|auth| auth.authorization_header_value()),
        Some("Bearer shared-secret".to_string())
    );
}

#[tokio::test]
async fn load_rejects_empty_http_bearer_auth() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[http_auth]
bearer_token = ""
"#,
            neutral_workdir(&dir)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("http_auth.bearer_token must not be empty"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_rejects_pinned_client_cert_for_http_transport() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    let daemon_cert = dir.path().join("daemon.pem");
    let daemon_key = dir.path().join("daemon.key");
    let ca_cert = dir.path().join("ca.pem");
    let broker_cert = dir.path().join("broker.pem");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
pinned_client_cert_pem = {}
"#,
            neutral_workdir(&dir),
            neutral_toml_path(&daemon_cert),
            neutral_toml_path(&daemon_key),
            neutral_toml_path(&ca_cert),
            neutral_toml_path(&broker_cert)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("pinned_client_cert_pem requires transport = \"tls\""),
        "unexpected error: {err}"
    );
}

#[cfg(not(feature = "tls"))]
#[tokio::test]
async fn load_rejects_explicit_tls_transport_when_tls_feature_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("daemon.toml");
    let daemon_cert = dir.path().join("daemon.pem");
    let daemon_key = dir.path().join("daemon.key");
    let ca_cert = dir.path().join("ca.pem");
    tokio::fs::write(
        &config_path,
        format!(
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
transport = "tls"

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
"#,
            neutral_workdir(&dir),
            neutral_toml_path(&daemon_cert),
            neutral_toml_path(&daemon_key),
            neutral_toml_path(&ca_cert)
        ),
    )
    .await
    .unwrap();

    let err = DaemonConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains(crate::tls::FEATURE_REQUIRED_MESSAGE),
        "unexpected error: {err}"
    );
}
