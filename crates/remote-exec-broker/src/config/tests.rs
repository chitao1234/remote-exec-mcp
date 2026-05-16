use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use crate::state::LOCAL_TARGET_NAME;

use super::{BrokerConfig, McpServerConfig, SseInterval, ValidatedBrokerConfig};

fn valid_target_config(name: &str) -> String {
    if cfg!(feature = "broker-tls") {
        format!(
            r#"[targets.{name}]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#
        )
    } else {
        format!(
            r#"[targets.{name}]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
"#
        )
    }
}

fn toml_string(path: &Path) -> toml::Value {
    toml::Value::String(path.display().to_string())
}

async fn load_config(
    dir: &tempfile::TempDir,
    text: impl AsRef<str>,
) -> anyhow::Result<ValidatedBrokerConfig> {
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(&config_path, text.as_ref()).await?;
    BrokerConfig::load(&config_path).await
}

#[tokio::test]
async fn load_rejects_reserved_local_target_name() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_config(&dir, valid_target_config(LOCAL_TARGET_NAME))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("configured target name `local` is reserved"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_accepts_non_reserved_target_names() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(&dir, valid_target_config("builder-a"))
        .await
        .unwrap();
    assert!(config.targets.contains_key("builder-a"));
}

#[tokio::test]
async fn load_accepts_remote_target_timeout_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
connect_ms = 1234
read_ms = 2345
request_ms = 3456
startup_probe_ms = 4567
"#,
    )
    .await
    .unwrap();
    let timeouts = config.targets["builder-a"].timeouts;
    assert_eq!(timeouts.connect_ms, 1234);
    assert_eq!(timeouts.read_ms, 2345);
    assert_eq!(timeouts.request_ms, 3456);
    assert_eq!(timeouts.startup_probe_ms, 4567);
    assert_eq!(
        timeouts.request_timeout(),
        std::time::Duration::from_millis(3456)
    );
}

#[tokio::test]
async fn load_rejects_zero_remote_target_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_config(
        &dir,
        r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
request_ms = 0
"#,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("target `builder-a` timeouts.request_ms must be greater than zero"),
        "unexpected error: {err}"
    );
}

#[cfg(not(feature = "broker-tls"))]
#[tokio::test]
async fn load_accepts_https_targets_even_when_broker_tls_feature_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        r#"[targets.builder-a]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        config.targets["builder-a"].base_url,
        "https://127.0.0.1:8443"
    );
}

#[tokio::test]
async fn load_accepts_local_only_broker_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        format!(
            "[local]\ndefault_workdir = {}\nallow_login_shell = false\n",
            toml_string(dir.path())
        ),
    )
    .await
    .unwrap();
    assert!(config.targets.is_empty());
    assert_eq!(
        config.local.as_ref().map(|local| &local.default_workdir),
        Some(&dir.path().to_path_buf())
    );
    assert_eq!(
        config.local.as_ref().map(|local| local.allow_login_shell),
        Some(false)
    );
    assert!(!config.disable_structured_content);
    assert!(matches!(config.mcp, McpServerConfig::Stdio));
}

#[tokio::test]
async fn load_rejects_missing_local_default_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let missing_workdir = dir.path().join("missing-local-workdir");
    let err = load_config(
        &dir,
        format!(
            "[local]\ndefault_workdir = {}\n",
            toml_string(&missing_workdir)
        ),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("local.default_workdir")
            && err.to_string().contains("does not exist"),
        "unexpected error: {err}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn load_accepts_local_windows_posix_root() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        format!(
            "[local]\ndefault_workdir = {}\nwindows_posix_root = \"C:\\\\msys64\"\n",
            toml_string(dir.path())
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        config
            .local
            .as_ref()
            .and_then(|local| local.windows_posix_root.as_ref()),
        Some(&PathBuf::from(r"C:\msys64"))
    );
}

#[cfg(windows)]
#[tokio::test]
async fn load_normalizes_local_default_workdir_through_windows_posix_root() {
    let dir = tempfile::tempdir().unwrap();
    let synthetic_root = dir.path().join("msys64");
    std::fs::create_dir_all(synthetic_root.join("tmp")).unwrap();
    let config = load_config(
        &dir,
        format!(
            "[local]\ndefault_workdir = \"/tmp\"\nwindows_posix_root = {}\n",
            toml_string(&synthetic_root)
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        config
            .local
            .as_ref()
            .map(|local| local.default_workdir.clone()),
        Some(synthetic_root.join("tmp"))
    );
}

#[tokio::test]
async fn load_accepts_local_apply_patch_encoding_autodetect() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        format!(
            "[local]\ndefault_workdir = {}\nexperimental_apply_patch_target_encoding_autodetect = true\n",
            toml_string(dir.path())
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        config
            .local
            .as_ref()
            .map(|local| local.experimental_apply_patch_target_encoding_autodetect),
        Some(true)
    );
}

#[tokio::test]
async fn load_accepts_disabling_structured_content() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config(
        &dir,
        format!(
            "disable_structured_content = true\n\n{}",
            valid_target_config("builder-a")
        ),
    )
    .await
    .unwrap();
    assert!(config.disable_structured_content);
}

#[tokio::test]
async fn bundled_broker_example_preserves_intentional_structured_content_override() {
    let example_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/broker.example.toml");
    let example_text = tokio::fs::read_to_string(&example_path).await.unwrap();

    let config: BrokerConfig = toml::from_str(&example_text).unwrap();
    config.validate().unwrap();
    assert!(config.disable_structured_content);
    assert!(matches!(config.mcp, McpServerConfig::Stdio));
}

#[tokio::test]
async fn load_rejects_http_target_without_explicit_insecure_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
expected_daemon_name = "builder-xp"
"#,
    )
    .await
    .unwrap();

    let err = BrokerConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("http:// targets require allow_insecure_http = true"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_accepts_http_target_with_explicit_insecure_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
expected_daemon_name = "builder-xp"
"#,
    )
    .await
    .unwrap();

    let config = BrokerConfig::load(&config_path).await.unwrap();
    assert!(config.targets["builder-xp"].allow_insecure_http);
    assert_eq!(
        config.targets["builder-xp"].base_url,
        "http://127.0.0.1:8181"
    );
    assert_eq!(
        config.targets["builder-xp"].expected_daemon_name.as_deref(),
        Some("builder-xp")
    );
}

#[tokio::test]
async fn load_accepts_http_bearer_auth_for_target() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
expected_daemon_name = "builder-xp"

[targets.builder-xp.http_auth]
bearer_token = "shared-secret"
"#,
    )
    .await
    .unwrap();

    let config = BrokerConfig::load(&config_path).await.unwrap();
    assert_eq!(
        config.targets["builder-xp"]
            .http_auth
            .as_ref()
            .map(|auth| auth.bearer_token.as_str()),
        Some("shared-secret")
    );
}

#[tokio::test]
async fn load_rejects_empty_http_bearer_auth_for_target() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-xp.http_auth]
bearer_token = ""
"#,
    )
    .await
    .unwrap();

    let err = BrokerConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("http_auth.bearer_token must not be empty"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_rejects_server_name_skip_for_http_target() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
skip_server_name_verification = true
"#,
    )
    .await
    .unwrap();

    let err = BrokerConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot set skip_server_name_verification"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_rejects_server_cert_pin_for_http_target() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
pinned_server_cert_pem = "/tmp/pin.pem"
"#,
    )
    .await
    .unwrap();

    let err = BrokerConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot set pinned_server_cert_pem"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn load_accepts_streamable_http_mcp_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "/rpc"
stateful = false
sse_keep_alive_ms = 0
sse_retry_ms = 1000
"#,
    )
    .await
    .unwrap();

    let config = BrokerConfig::load(&config_path).await.unwrap();
    match &config.mcp {
        McpServerConfig::StreamableHttp {
            listen,
            path,
            stateful,
            sse_keep_alive,
            sse_retry,
        } => {
            assert_eq!(*listen, "127.0.0.1:8787".parse().unwrap());
            assert_eq!(path, "/rpc");
            assert!(!stateful);
            assert_eq!(*sse_keep_alive, SseInterval::Disabled);
            assert_eq!(
                *sse_retry,
                SseInterval::Duration(std::time::Duration::from_millis(1000))
            );
        }
        other => panic!("unexpected MCP config: {other:?}"),
    }
}

#[tokio::test]
async fn load_rejects_streamable_http_path_without_leading_slash() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broker.toml");
    tokio::fs::write(
        &config_path,
        r#"
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "mcp"
"#,
    )
    .await
    .unwrap();

    let err = BrokerConfig::load(&config_path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("streamable_http MCP path must start with `/`"),
        "unexpected error: {err}"
    );
}
