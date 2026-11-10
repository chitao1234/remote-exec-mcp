use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::public::{
    ApplyPatchInput, EditInput, ForwardPortSpec, ForwardPortsInput, ReadInput, TransferEndpoint,
    ViewImageInput, WriteInput, WriteStdinInput,
};
use remote_exec_proto::rpc::ExecPtySize;
use tokio::io::AsyncReadExt;

pub struct WriteStdinCliInput {
    pub first: String,
    pub second: Option<String>,
    pub chars: Option<String>,
    pub chars_file: Option<PathBuf>,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub pty_rows: Option<u16>,
    pub pty_cols: Option<u16>,
}

pub async fn build_write_stdin_input(args: WriteStdinCliInput) -> anyhow::Result<WriteStdinInput> {
    let (target, session_id, positional_chars) = if is_public_session_id(&args.first) {
        anyhow::ensure!(
            args.chars.is_none(),
            "too many positional arguments for `write-stdin`"
        );
        (None, args.first, args.second)
    } else {
        let session_id = args
            .second
            .context("write-stdin requires `<SESSION_ID>` or `<TARGET> <SESSION_ID>`")?;
        anyhow::ensure!(
            is_public_session_id(&session_id),
            "`{session_id}` is not a public session id (expected `sess_` followed by 32 hex characters)"
        );
        (Some(args.first), session_id, args.chars)
    };

    anyhow::ensure!(
        positional_chars.is_none() || args.chars_file.is_none(),
        "provide either inline chars or --chars-file, not both"
    );
    let chars = match positional_chars {
        Some(chars) => Some(chars),
        None => load_optional_text_input(None, args.chars_file).await?,
    };

    Ok(WriteStdinInput {
        session_id,
        chars,
        yield_time_ms: args.yield_time_ms,
        max_output_tokens: args.max_output_tokens,
        pty_size: write_stdin_pty_size(args.pty_rows, args.pty_cols)?,
        target,
    })
}

pub fn is_public_session_id(value: &str) -> bool {
    value.strip_prefix("sess_").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        input: load_text_input_or_stdin(input, input_file).await?,
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

pub fn build_read_input(
    target: String,
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
) -> ReadInput {
    ReadInput {
        target,
        file_path: path,
        offset,
        limit,
    }
}

pub async fn build_write_input(
    target: String,
    path: String,
    content: Option<String>,
    content_file: Option<PathBuf>,
) -> anyhow::Result<WriteInput> {
    Ok(WriteInput {
        target,
        file_path: path,
        content: load_text_input_or_stdin(content, content_file).await?,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn build_edit_input(
    target: String,
    path: String,
    old_string: Option<String>,
    old_string_file: Option<PathBuf>,
    new_string: Option<String>,
    new_string_file: Option<PathBuf>,
    replace_all: bool,
) -> anyhow::Result<EditInput> {
    Ok(EditInput {
        target,
        file_path: path,
        old_string: load_required_text_input(old_string, old_string_file).await?,
        new_string: load_optional_text_input(new_string, new_string_file)
            .await?
            .unwrap_or_default(),
        replace_all,
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
        forward_ids: forward_ids.into_iter().map(ForwardId::new).collect(),
    }
}

pub fn build_forward_ports_close_input(forward_ids: Vec<String>) -> ForwardPortsInput {
    ForwardPortsInput::Close {
        forward_ids: forward_ids.into_iter().map(ForwardId::new).collect(),
    }
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

pub async fn load_text_input_or_stdin(
    inline: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<String> {
    match load_optional_text_input(inline, file).await? {
        Some(text) => Ok(text),
        None => read_text_path(Path::new("-")).await,
    }
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
