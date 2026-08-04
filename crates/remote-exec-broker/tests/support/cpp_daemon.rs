use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use remote_exec_broker::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
use tempfile::TempDir;

use super::tunnel_drop_proxy::{TunnelDropProxy, TunnelDropProxyOptions};

#[cfg(feature = "broker-tls")]
use super::certs::TestCerts;

pub const CPP_TARGET: &str = "builder-cpp";

const CPP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CPP_READY_POLL: Duration = Duration::from_millis(50);
static MISSING_CPP_DAEMON_WARNING: Once = Once::new();

pub struct CppDaemonBrokerFixture {
    _tempdir: TempDir,
    #[cfg(feature = "broker-tls")]
    _cert_tempdir: Option<TempDir>,
    pub client: RemoteExecClient,
    proxy: TunnelDropProxy,
    daemon: tokio::process::Child,
    daemon_workdir: PathBuf,
    local_workdir: PathBuf,
}

impl CppDaemonBrokerFixture {
    pub async fn spawn() -> Option<Self> {
        Self::spawn_with_daemon_config("").await
    }

    pub async fn spawn_with_daemon_config(extra_daemon_config: &str) -> Option<Self> {
        Self::spawn_with_transport(extra_daemon_config, None).await
    }

    #[cfg(feature = "broker-tls")]
    pub async fn spawn_tls() -> Option<Self> {
        Self::spawn_tls_with_daemon_config("").await
    }

    #[cfg(feature = "broker-tls")]
    pub async fn spawn_tls_with_daemon_config(extra_daemon_config: &str) -> Option<Self> {
        let cert_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../remote-exec-daemon-cpp/tests/fixtures/tls");
        let certs = TestCerts {
            ca_cert: cert_dir.join("ca.pem"),
            client_cert: cert_dir.join("client.pem"),
            client_key: cert_dir.join("client.key"),
            daemon_cert: cert_dir.join("server.pem"),
            daemon_key: cert_dir.join("server.key"),
        };
        Self::spawn_with_transport(extra_daemon_config, Some((None, certs))).await
    }

    async fn spawn_with_transport(
        extra_daemon_config: &str,
        #[cfg(feature = "broker-tls")] tls: Option<(Option<TempDir>, TestCerts)>,
        #[cfg(not(feature = "broker-tls"))] _tls: Option<()>,
    ) -> Option<Self> {
        let daemon_binary = match cpp_daemon_binary() {
            Some(path) => path,
            None => {
                warn_missing_cpp_daemon_executable();
                return None;
            }
        };
        remote_exec_broker::install_crypto_provider().unwrap();

        #[cfg(feature = "broker-tls")]
        let (cert_tempdir, certs) = tls
            .map(|(tempdir, certs)| (tempdir, Some(certs)))
            .unwrap_or((None, None));
        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(&daemon_binary, tempdir.path());
        let broker_config = tempdir.path().join("broker.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        let local_workdir = tempdir.path().join("local-work");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        std::fs::create_dir_all(&local_workdir).unwrap();

        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        #[cfg(feature = "broker-tls")]
        let tls_config = certs
            .as_ref()
            .map(|certs| {
                format!(
                    "transport = tls\ntls_cert_pem = {}\ntls_key_pem = {}\ntls_ca_pem = {}\n",
                    cpp_config_path(&certs.daemon_cert),
                    cpp_config_path(&certs.daemon_key),
                    cpp_config_path(&certs.ca_cert),
                )
            })
            .unwrap_or_default();
        #[cfg(not(feature = "broker-tls"))]
        let tls_config = String::new();
        let daemon_config_body = format!(
            "target = {CPP_TARGET}\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n{}{}",
            cpp_config_path(&daemon_workdir),
            cpp_config_path(&daemon_bound_addr_file),
            tls_config,
            extra_daemon_config,
        );
        let (daemon, backend_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
            #[cfg(feature = "broker-tls")]
            certs.is_none(),
            #[cfg(not(feature = "broker-tls"))]
            true,
        )
        .await;
        #[cfg(feature = "broker-tls")]
        let raw_stream = certs.is_some();
        #[cfg(not(feature = "broker-tls"))]
        let raw_stream = false;
        let proxy = TunnelDropProxy::spawn(
            backend_addr,
            TunnelDropProxyOptions {
                raw_stream,
                rewrite_plain_requests: false,
                panic_context: "C++ tunnel-drop proxy",
            },
        )
        .await;
        let daemon_addr = proxy.listen_addr;

        #[cfg(feature = "broker-tls")]
        let broker_target = if let Some(certs) = certs.as_ref() {
            format!(
                "base_url = \"https://localhost:{}\"\nca_pem = {}\nclient_cert_pem = {}\nclient_key_pem = {}\nskip_server_name_verification = true\nexpected_daemon_name = \"{CPP_TARGET}\"",
                daemon_addr.port(),
                super::test_helpers::toml_string(&certs.ca_cert.display().to_string()),
                super::test_helpers::toml_string(&certs.client_cert.display().to_string()),
                super::test_helpers::toml_string(&certs.client_key.display().to_string()),
            )
        } else {
            format!(
                "base_url = \"http://{daemon_addr}\"\nallow_insecure_http = true\nexpected_daemon_name = \"{CPP_TARGET}\""
            )
        };
        #[cfg(not(feature = "broker-tls"))]
        let broker_target = format!(
            "base_url = \"http://{daemon_addr}\"\nallow_insecure_http = true\nexpected_daemon_name = \"{CPP_TARGET}\""
        );

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.{target}]
{broker_target}

[local]
default_workdir = {local_workdir}
pty = "none"
"#,
                target = CPP_TARGET,
                broker_target = broker_target,
                local_workdir =
                    super::test_helpers::toml_string(&local_workdir.display().to_string()),
            ),
        )
        .unwrap();

        let client = RemoteExecClient::connect(Connection::Config {
            config_path: broker_config,
        })
        .await
        .unwrap();

        Some(Self {
            _tempdir: tempdir,
            #[cfg(feature = "broker-tls")]
            _cert_tempdir: cert_tempdir,
            client,
            proxy,
            daemon,
            daemon_workdir,
            local_workdir,
        })
    }

    pub fn daemon_workdir(&self) -> &Path {
        &self.daemon_workdir
    }

    pub fn local_workdir(&self) -> &Path {
        &self.local_workdir
    }

    pub async fn open_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
    ) -> ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Open {
                    listen_side: listen_side.to_string(),
                    connect_side: connect_side.to_string(),
                    forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                        listen_endpoint,
                        connect_endpoint,
                        protocol,
                    }],
                },
            )
            .await
            .unwrap()
    }

    pub async fn open_tcp_forward(&self, connect_endpoint: &str) -> ToolResponse {
        self.open_forward(
            CPP_TARGET,
            "local",
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    pub async fn open_tcp_forward_local_to_cpp(&self, connect_endpoint: &str) -> ToolResponse {
        self.open_forward(
            "local",
            CPP_TARGET,
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    pub async fn close_forward(&self, forward_id: String) -> ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Close {
                    forward_ids: vec![forward_id.into()],
                },
            )
            .await
            .unwrap()
    }

    pub async fn drop_port_tunnels(&self) {
        self.proxy.drop_port_tunnels().await;
        self.proxy.assert_no_task_panics().await;
    }
}

impl Drop for CppDaemonBrokerFixture {
    fn drop(&mut self) {
        self.proxy.stop();
        let _ = self.daemon.start_kill();
    }
}

pub async fn wait_for_forward_ready(
    client: &RemoteExecClient,
    forward_id: &str,
    timeout: Duration,
) -> serde_json::Value {
    let started = std::time::Instant::now();
    loop {
        let response = client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::List {
                    forward_ids: vec![ForwardId::new(forward_id)],
                    listen_side: None,
                    connect_side: None,
                },
            )
            .await
            .unwrap();
        let entry = response.structured_content["forwards"][0].clone();
        if entry["status"] == "open" && entry["phase"] == "ready" {
            return entry;
        }
        if started.elapsed() >= timeout {
            panic!("forward `{forward_id}` did not become ready within {timeout:?}; last={entry}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub struct CrashableCppDaemonBrokerFixture {
    _tempdir: TempDir,
    broker_config: PathBuf,
    pub client: RemoteExecClient,
    broker: tokio::process::Child,
    daemon: tokio::process::Child,
}

impl CrashableCppDaemonBrokerFixture {
    pub async fn spawn() -> Option<Self> {
        let daemon_binary = match cpp_daemon_binary() {
            Some(path) => path,
            None => {
                warn_missing_cpp_daemon_executable();
                return None;
            }
        };
        remote_exec_broker::install_crypto_provider().unwrap();

        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(&daemon_binary, tempdir.path());
        let broker_config = tempdir.path().join("broker-http.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        let local_workdir = tempdir.path().join("local-work");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        std::fs::create_dir_all(&local_workdir).unwrap();
        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let daemon_config_body = format!(
            "target = {CPP_TARGET}\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n",
            cpp_config_path(&daemon_workdir),
            cpp_config_path(&daemon_bound_addr_file)
        );
        let (daemon, daemon_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
            true,
        )
        .await;

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.{target}]
base_url = "http://{daemon_addr}"
allow_insecure_http = true
expected_daemon_name = "{target}"

[local]
default_workdir = {local_workdir}
pty = "none"

[mcp]
transport = "streamable_http"
listen = {broker_listen}
path = "/mcp"
"#,
                target = CPP_TARGET,
                daemon_addr = daemon_addr,
                local_workdir =
                    super::test_helpers::toml_string(&local_workdir.display().to_string()),
                broker_listen = super::test_helpers::toml_string("127.0.0.1:0"),
            ),
        )
        .unwrap();

        let mut broker = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        broker.arg(&broker_config);
        super::apply_quiet_test_logging(&mut broker);
        super::streamable_http_child::configure_streamable_http_broker_child(&mut broker);
        broker.kill_on_drop(true);
        let mut broker = broker.spawn().unwrap();
        let broker_addr = super::streamable_http_child::wait_for_streamable_http_bound_addr(
            &mut broker,
            "C++ broker",
        )
        .await;
        let broker_url = format!("http://{broker_addr}/mcp");
        wait_until_ready_mcp_http(&broker_url).await;
        let client = RemoteExecClient::connect(Connection::StreamableHttp { url: broker_url })
            .await
            .unwrap();

        Some(Self {
            _tempdir: tempdir,
            broker_config,
            client,
            broker,
            daemon,
        })
    }

    pub async fn kill_broker(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.broker.wait().await;
    }

    pub async fn wait_for_public_forward_reopen(
        &self,
        endpoint: &str,
        timeout: Duration,
    ) -> (RemoteExecClient, String) {
        const CPP_PUBLIC_REOPEN_POLL: Duration = Duration::from_millis(200);

        let started = std::time::Instant::now();
        loop {
            let client = RemoteExecClient::connect(Connection::Config {
                config_path: self.broker_config.clone(),
            })
            .await
            .unwrap();
            let response = client
                .call_tool(
                    "forward_ports",
                    &ForwardPortsInput::Open {
                        listen_side: CPP_TARGET.to_string(),
                        connect_side: "local".to_string(),
                        forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                            listen_endpoint: endpoint.to_string(),
                            connect_endpoint: "127.0.0.1:9".to_string(),
                            protocol: ForwardPortProtocol::Tcp,
                        }],
                    },
                )
                .await
                .unwrap();
            if !response.is_error {
                assert_eq!(
                    response.structured_content["forwards"][0]["listen_endpoint"],
                    endpoint
                );
                let forward_id = response.structured_content["forwards"][0]["forward_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                return (client, forward_id);
            }
            if !response
                .text_output
                .contains("opening tcp listener on `builder-cpp`")
            {
                panic!(
                    "unexpected public reopen failure while waiting for {endpoint}: {}",
                    response.text_output
                );
            }
            if started.elapsed() >= timeout {
                panic!(
                    "C++ daemon listener on {endpoint} was not released within {timeout:?}; last error={}",
                    response.text_output
                );
            }
            tokio::time::sleep(CPP_PUBLIC_REOPEN_POLL).await;
        }
    }
}

impl Drop for CrashableCppDaemonBrokerFixture {
    fn drop(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.daemon.start_kill();
    }
}

fn cpp_daemon_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("REMOTE_EXEC_CPP_DAEMON").map(PathBuf::from) {
        return path.exists().then_some(path);
    }

    cpp_daemon_default_binaries()
        .into_iter()
        .find(|path| path.exists())
}

fn cpp_daemon_default_binaries() -> Vec<PathBuf> {
    let daemon_dir = cpp_daemon_dir();
    if cfg!(windows) {
        vec![
            daemon_dir.join("build/msvc-native/remote-exec-daemon-cpp-msvc.exe"),
            daemon_dir.join("build/msvc-xp/remote-exec-daemon-cpp-xp-msvc.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-native-xp-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-xp-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-2000-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-nt4-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-nt4-ws1.exe"),
        ]
    } else {
        vec![daemon_dir.join("build/remote-exec-daemon-cpp")]
    }
}

fn cpp_daemon_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../remote-exec-daemon-cpp")
}

fn stage_cpp_daemon_binary(source: &Path, tempdir: &Path) -> PathBuf {
    let staged_name = if cfg!(windows) {
        "remote-exec-daemon-cpp.exe"
    } else {
        "remote-exec-daemon-cpp"
    };
    let staged = tempdir.join(staged_name);
    std::fs::copy(source, &staged).unwrap();
    stage_cpp_daemon_companion(source, tempdir, "winpty-agent.exe");
    staged
}

fn stage_cpp_daemon_companion(source: &Path, tempdir: &Path, file_name: &str) {
    let Some(source_dir) = source.parent() else {
        return;
    };
    let companion = source_dir.join(file_name);
    if companion.exists() {
        std::fs::copy(&companion, tempdir.join(file_name)).unwrap();
    }
}

async fn spawn_cpp_daemon_process(command: &mut tokio::process::Command) -> tokio::process::Child {
    const ETXTBSY: i32 = 26;
    for attempt in 0..5 {
        match command.spawn() {
            Ok(child) => return child,
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempt + 1 < 5 => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("failed to spawn staged C++ daemon: {error}"),
        }
    }
    unreachable!("spawn retry loop returns or panics");
}

async fn wait_for_bound_addr_file(path: &Path, resource: &str) -> std::net::SocketAddr {
    let started = std::time::Instant::now();
    loop {
        let last = match tokio::fs::read_to_string(path).await {
            Ok(value) => match value.trim().parse() {
                Ok(addr) => return addr,
                Err(err) => format!("invalid address `{}`: {err}", value.trim()),
            },
            Err(err) => err.to_string(),
        };
        if started.elapsed() >= Duration::from_secs(5) {
            panic!(
                "{resource} did not write bound address file {}; last={last}",
                path.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn spawn_cpp_daemon_with_bound_addr(
    daemon_binary: &Path,
    daemon_config: &Path,
    bound_addr_file: &Path,
    config_body: String,
    wait_for_http_ready: bool,
) -> (tokio::process::Child, std::net::SocketAddr) {
    std::fs::write(daemon_config, config_body).unwrap();
    let mut daemon = tokio::process::Command::new(daemon_binary);
    daemon.arg(daemon_config);
    super::apply_quiet_test_logging(&mut daemon);
    let child = spawn_cpp_daemon_process(&mut daemon).await;
    let daemon_addr = wait_for_bound_addr_file(bound_addr_file, "C++ daemon").await;
    if wait_for_http_ready {
        wait_until_ready_http(daemon_addr).await;
    }
    (child, daemon_addr)
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    super::wait_until_ready_http(addr, CPP_READY_TIMEOUT, CPP_READY_POLL, "real C++ daemon").await;
}

async fn wait_until_ready_mcp_http(url: &str) {
    super::wait_until_ready_mcp_http(
        url,
        CPP_READY_TIMEOUT,
        CPP_READY_POLL,
        "broker MCP HTTP endpoint",
    )
    .await;
}

fn cpp_config_path(path: &Path) -> String {
    path.display().to_string()
}

fn cpp_daemon_skip_message() -> String {
    let default = cpp_daemon_default_binaries()
        .into_iter()
        .next()
        .expect("at least one default C++ daemon path");
    format!(
        "set REMOTE_EXEC_CPP_DAEMON or build {} before running this test",
        default.display()
    )
}

fn warn_missing_cpp_daemon_executable() {
    MISSING_CPP_DAEMON_WARNING.call_once(|| {
        let env_path = std::env::var_os("REMOTE_EXEC_CPP_DAEMON")
            .map(PathBuf::from)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<not set>".to_string());
        let default_paths = cpp_daemon_default_binaries()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "\n\
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\
!!! WARNING: SKIPPING REAL C++ DAEMON INTEGRATION TESTS                 !!!\n\
!!!                                                                        !!!\n\
!!! The remote-exec-daemon-cpp executable is not available. These Rust     !!!\n\
!!! integration tests are being skipped instead of exercising the real     !!!\n\
!!! C++ daemon.                                                           !!!\n\
!!!                                                                        !!!\n\
!!! REMOTE_EXEC_CPP_DAEMON = {env_path}\n\
!!! default checked paths    = {default_paths}\n\
!!!                                                                        !!!\n\
!!! {skip_message}\n\
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n",
            default_paths = default_paths,
            skip_message = cpp_daemon_skip_message()
        );
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(message.as_bytes());
        let _ = stderr.flush();
    });
}
