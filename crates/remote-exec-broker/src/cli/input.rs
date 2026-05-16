use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_proto::public::{
    ApplyPatchInput, ExecCommandInput, ForwardPortSpec, ForwardPortsInput, TransferEndpoint,
    TransferFilesInput, ViewImageInput, WriteStdinInput,
};
use remote_exec_proto::rpc::ExecPtySize;
use tokio::io::AsyncReadExt;

#[allow(clippy::too_many_arguments, reason = "CLI field passthrough helper")]
pub fn build_exec_command_input(
    target: String,
    cmd: String,
    workdir: Option<String>,
    shell: Option<String>,
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    login: Option<bool>,
) -> ExecCommandInput {
    ExecCommandInput {
        target,
        cmd,
        workdir,
        shell,
        tty,
        yield_time_ms,
        max_output_tokens,
        login,
    }
}

#[allow(clippy::too_many_arguments, reason = "CLI field passthrough helper")]
pub async fn build_write_stdin_input(
    session_id: String,
    chars: Option<String>,
    chars_file: Option<PathBuf>,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<u32>,
    pty_rows: Option<u16>,
    pty_cols: Option<u16>,
    target: Option<String>,
) -> anyhow::Result<WriteStdinInput> {
    Ok(WriteStdinInput {
        session_id,
        chars: load_optional_text_input(chars, chars_file).await?,
        yield_time_ms,
        max_output_tokens,
        pty_size: write_stdin_pty_size(pty_rows, pty_cols)?,
        target,
    })
}

pub fn write_stdin_pty_size(
    rows: Option<u16>,
    cols: Option<u16>,
) -> anyhow::Result<Option<ExecPtySize>> {
    match (rows, cols) {
        (None, None) => Ok(None),
        (Some(rows), Some(cols)) if rows > 0 && cols > 0 => Ok(Some(ExecPtySize { rows, cols })),
        (Some(_), Some(_)) => anyhow::bail!("--pty-rows and --pty-cols must be greater than zero"),
        _ => anyhow::bail!("--pty-rows and --pty-cols must be provided together"),
    }
}

pub async fn build_apply_patch_input(
    target: String,
    input: Option<String>,
    input_file: Option<PathBuf>,
    workdir: Option<String>,
) -> anyhow::Result<ApplyPatchInput> {
    Ok(ApplyPatchInput {
        target,
        input: load_required_text_input(input, input_file).await?,
        workdir,
    })
}

pub fn build_view_image_input(
    target: String,
    path: String,
    workdir: Option<String>,
    detail: Option<String>,
) -> ViewImageInput {
    ViewImageInput {
        target,
        path,
        workdir,
        detail,
    }
}

pub fn build_transfer_files_input(
    sources: &[String],
    destination: &str,
    exclude: Vec<String>,
    overwrite: remote_exec_proto::public::TransferOverwrite,
    destination_mode: remote_exec_proto::public::TransferDestinationMode,
    symlink_mode: remote_exec_proto::public::TransferSymlinkMode,
    create_parent: bool,
) -> anyhow::Result<TransferFilesInput> {
    let endpoints = sources
        .iter()
        .map(|endpoint| parse_transfer_endpoint(endpoint))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(TransferFilesInput {
        source: None,
        sources: endpoints,
        destination: parse_transfer_endpoint(destination)?,
        exclude,
        overwrite,
        destination_mode,
        symlink_mode,
        create_parent,
    })
}

pub fn build_forward_ports_open_input(
    listen_side: String,
    connect_side: String,
    forwards: &[String],
) -> anyhow::Result<ForwardPortsInput> {
    Ok(ForwardPortsInput::Open {
        listen_side,
        connect_side,
        forwards: forwards
            .iter()
            .map(|value| parse_forward_spec(value))
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
}

pub fn build_forward_ports_list_input(
    listen_side: Option<String>,
    connect_side: Option<String>,
    forward_ids: Vec<String>,
) -> ForwardPortsInput {
    ForwardPortsInput::List {
        listen_side,
        connect_side,
        forward_ids,
    }
}

pub fn build_forward_ports_close_input(forward_ids: Vec<String>) -> ForwardPortsInput {
    ForwardPortsInput::Close { forward_ids }
}

pub fn resolve_login_flag(login: bool, no_login: bool) -> Option<bool> {
    match (login, no_login) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

pub async fn load_required_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<String> {
    load_optional_text_input(inline, file)
        .await?
        .context("missing required text input")
}

pub async fn load_optional_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<Option<String>> {
    match (inline, file) {
        (Some(text), None) => Ok(Some(text)),
        (None, Some(path)) => Ok(Some(read_text_path(&path).await?)),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => anyhow::bail!("provide either inline text or a file path, not both"),
    }
}

async fn read_text_path(path: &Path) -> anyhow::Result<String> {
    let bytes = if path == Path::new("-") {
        let mut bytes = Vec::new();
        tokio::io::stdin()
            .read_to_end(&mut bytes)
            .await
            .context("reading stdin")?;
        bytes
    } else {
        tokio::fs::read(path)
            .await
            .with_context(|| format!("reading {}", path.display()))?
    };

    String::from_utf8(bytes).context("text input was not valid UTF-8")
}

pub fn parse_transfer_endpoint(value: &str) -> anyhow::Result<TransferEndpoint> {
    let (target, path) = value
        .split_once(':')
        .with_context(|| format!("invalid endpoint `{value}`; expected <target>:<path>"))?;
    anyhow::ensure!(!target.is_empty(), "endpoint target must not be empty");
    anyhow::ensure!(!path.is_empty(), "endpoint path must not be empty");

    Ok(TransferEndpoint {
        target: target.to_string(),
        path: path.to_string(),
    })
}

pub fn parse_forward_spec(value: &str) -> anyhow::Result<ForwardPortSpec> {
    let (protocol, endpoints) = value.split_once(':').with_context(|| {
        format!("invalid forward `{value}`; expected <protocol>:<listen>=<connect>")
    })?;
    let (listen_endpoint, connect_endpoint) = endpoints.split_once('=').with_context(|| {
        format!("invalid forward `{value}`; expected <protocol>:<listen>=<connect>")
    })?;
    let protocol = match protocol {
        "tcp" => remote_exec_proto::public::ForwardPortProtocol::Tcp,
        "udp" => remote_exec_proto::public::ForwardPortProtocol::Udp,
        other => anyhow::bail!("unsupported forward protocol `{other}`"),
    };
    anyhow::ensure!(
        !listen_endpoint.is_empty(),
        "forward listen endpoint must not be empty"
    );
    anyhow::ensure!(
        !connect_endpoint.is_empty(),
        "forward connect endpoint must not be empty"
    );

    Ok(ForwardPortSpec {
        listen_endpoint: listen_endpoint.to_string(),
        connect_endpoint: connect_endpoint.to_string(),
        protocol,
    })
}
