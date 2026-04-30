use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use remote_exec_broker::client::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::public::{
    ApplyPatchInput, ExecCommandInput, ForwardPortProtocol, ForwardPortSpec, ForwardPortsInput,
    ListTargetsInput, TransferEndpoint, TransferFilesInput, TransferOverwrite, ViewImageInput,
    WriteStdinInput,
};
use tokio::io::AsyncReadExt;

#[derive(Parser, Debug)]
#[command(name = "remote-exec")]
#[command(about = "CLI client for a remote-exec-mcp broker")]
struct Cli {
    #[command(flatten)]
    connection: ConnectionArgs,

    #[arg(long, default_value_t = false)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("connection")
        .required(true)
        .args(["broker_config", "broker_url"])
))]
struct ConnectionArgs {
    #[arg(long)]
    broker_config: Option<PathBuf>,

    #[arg(long)]
    broker_url: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    ListTargets,
    #[command(name = "exec-command")]
    Exec(ExecCommandArgs),
    WriteStdin(WriteStdinArgs),
    ApplyPatch(ApplyPatchArgs),
    ViewImage(ViewImageArgs),
    TransferFiles(TransferFilesArgs),
    ForwardPorts(ForwardPortsArgs),
}

#[derive(Args, Debug)]
struct ExecCommandArgs {
    #[arg(long)]
    target: String,

    cmd: String,

    #[arg(long)]
    workdir: Option<String>,

    #[arg(long)]
    shell: Option<String>,

    #[arg(long, default_value_t = false)]
    tty: bool,

    #[arg(long)]
    yield_time_ms: Option<u64>,

    #[arg(long)]
    max_output_tokens: Option<u32>,

    #[arg(long, default_value_t = false, overrides_with = "no_login")]
    login: bool,

    #[arg(long, default_value_t = false, overrides_with = "login")]
    no_login: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("input").args(["chars", "chars_file"])))]
struct WriteStdinArgs {
    #[arg(long)]
    session_id: String,

    #[arg(long)]
    chars: Option<String>,

    #[arg(long)]
    chars_file: Option<PathBuf>,

    #[arg(long)]
    yield_time_ms: Option<u64>,

    #[arg(long)]
    max_output_tokens: Option<u32>,

    #[arg(long)]
    target: Option<String>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("patch_input")
        .required(true)
        .args(["input", "input_file"])
))]
struct ApplyPatchArgs {
    #[arg(long)]
    target: String,

    #[arg(long)]
    workdir: Option<String>,

    #[arg(long)]
    input: Option<String>,

    #[arg(long)]
    input_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ViewImageArgs {
    #[arg(long)]
    target: String,

    #[arg(long)]
    path: String,

    #[arg(long)]
    workdir: Option<String>,

    #[arg(long)]
    detail: Option<String>,

    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct TransferFilesArgs {
    #[arg(long = "source", required = true)]
    sources: Vec<String>,

    #[arg(long)]
    destination: String,

    #[arg(long, value_enum, default_value_t = CliTransferOverwrite::Fail)]
    overwrite: CliTransferOverwrite,

    #[arg(long, default_value_t = false)]
    create_parent: bool,
}

#[derive(Args, Debug)]
struct ForwardPortsArgs {
    #[command(subcommand)]
    action: ForwardPortsActionArgs,
}

#[derive(Subcommand, Debug)]
enum ForwardPortsActionArgs {
    Open(ForwardPortsOpenArgs),
    List(ForwardPortsListArgs),
    Close(ForwardPortsCloseArgs),
}

#[derive(Args, Debug)]
struct ForwardPortsOpenArgs {
    #[arg(long)]
    listen_side: String,

    #[arg(long)]
    connect_side: String,

    #[arg(long = "forward", required = true)]
    forwards: Vec<String>,
}

#[derive(Args, Debug)]
struct ForwardPortsListArgs {
    #[arg(long)]
    listen_side: Option<String>,

    #[arg(long)]
    connect_side: Option<String>,

    #[arg(long = "forward-id")]
    forward_ids: Vec<String>,
}

#[derive(Args, Debug)]
struct ForwardPortsCloseArgs {
    #[arg(long = "forward-id", required = true)]
    forward_ids: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliTransferOverwrite {
    Fail,
    Replace,
}

impl From<CliTransferOverwrite> for TransferOverwrite {
    fn from(value: CliTransferOverwrite) -> Self {
        match value {
            CliTransferOverwrite::Fail => Self::Fail,
            CliTransferOverwrite::Replace => Self::Replace,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let connection = cli.connection.resolve()?;
    let client = RemoteExecClient::connect(connection).await?;
    let exit_code = run_command(&client, cli.command, cli.json).await?;

    std::process::exit(exit_code);
}

async fn run_command(
    client: &RemoteExecClient,
    command: Command,
    json: bool,
) -> anyhow::Result<i32> {
    match command {
        Command::ListTargets => run_list_targets(client, json).await,
        Command::Exec(args) => run_exec(client, args, json).await,
        Command::WriteStdin(args) => run_write_stdin(client, args, json).await,
        Command::ApplyPatch(args) => run_apply_patch(client, args, json).await,
        Command::ViewImage(args) => run_view_image(client, args, json).await,
        Command::TransferFiles(args) => run_transfer_files(client, args, json).await,
        Command::ForwardPorts(args) => run_forward_ports(client, args, json).await,
    }
}

async fn run_list_targets(client: &RemoteExecClient, json: bool) -> anyhow::Result<i32> {
    let response = client
        .call_tool("list_targets", &ListTargetsInput::default())
        .await?;
    emit_and_status(&response, json)
}

async fn run_exec(
    client: &RemoteExecClient,
    args: ExecCommandArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let response = client
        .call_tool("exec_command", &exec_command_input(args))
        .await?;
    emit_and_status(&response, json)
}

async fn run_write_stdin(
    client: &RemoteExecClient,
    args: WriteStdinArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let response = client
        .call_tool("write_stdin", &write_stdin_input(args).await?)
        .await?;
    emit_and_status(&response, json)
}

async fn run_apply_patch(
    client: &RemoteExecClient,
    args: ApplyPatchArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let response = client
        .call_tool("apply_patch", &apply_patch_input(args).await?)
        .await?;
    emit_and_status(&response, json)
}

async fn run_view_image(
    client: &RemoteExecClient,
    args: ViewImageArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let output_path = args.out.clone();
    let response = client
        .call_tool("view_image", &view_image_input(args))
        .await?;
    if !response.is_error {
        if let Some(out) = &output_path {
            write_image_output(&response, out).await?;
        }
    }
    emit_view_image_response(&response, json, output_path.as_deref())?;
    Ok(status_code(&response))
}

async fn run_transfer_files(
    client: &RemoteExecClient,
    args: TransferFilesArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let response = client
        .call_tool("transfer_files", &transfer_files_input(args)?)
        .await?;
    emit_and_status(&response, json)
}

async fn run_forward_ports(
    client: &RemoteExecClient,
    args: ForwardPortsArgs,
    json: bool,
) -> anyhow::Result<i32> {
    let response = client
        .call_tool("forward_ports", &forward_ports_input(args)?)
        .await?;
    emit_and_status(&response, json)
}

fn exec_command_input(args: ExecCommandArgs) -> ExecCommandInput {
    ExecCommandInput {
        target: args.target,
        cmd: args.cmd,
        workdir: args.workdir,
        shell: args.shell,
        tty: args.tty,
        yield_time_ms: args.yield_time_ms,
        max_output_tokens: args.max_output_tokens,
        login: resolve_login_flag(args.login, args.no_login),
    }
}

async fn write_stdin_input(args: WriteStdinArgs) -> anyhow::Result<WriteStdinInput> {
    Ok(WriteStdinInput {
        session_id: args.session_id,
        chars: load_optional_text_input(args.chars, args.chars_file).await?,
        yield_time_ms: args.yield_time_ms,
        max_output_tokens: args.max_output_tokens,
        target: args.target,
    })
}

async fn apply_patch_input(args: ApplyPatchArgs) -> anyhow::Result<ApplyPatchInput> {
    Ok(ApplyPatchInput {
        target: args.target,
        input: load_required_text_input(args.input, args.input_file).await?,
        workdir: args.workdir,
    })
}

fn view_image_input(args: ViewImageArgs) -> ViewImageInput {
    ViewImageInput {
        target: args.target,
        path: args.path,
        workdir: args.workdir,
        detail: args.detail,
    }
}

fn transfer_files_input(args: TransferFilesArgs) -> anyhow::Result<TransferFilesInput> {
    let endpoints = args
        .sources
        .iter()
        .map(|endpoint| parse_transfer_endpoint(endpoint))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(TransferFilesInput {
        source: (endpoints.len() == 1).then(|| endpoints[0].clone()),
        sources: if endpoints.len() == 1 {
            Vec::new()
        } else {
            endpoints
        },
        destination: parse_transfer_endpoint(&args.destination)?,
        overwrite: args.overwrite.into(),
        create_parent: args.create_parent,
    })
}

fn forward_ports_input(args: ForwardPortsArgs) -> anyhow::Result<ForwardPortsInput> {
    Ok(match args.action {
        ForwardPortsActionArgs::Open(args) => ForwardPortsInput::Open {
            listen_side: args.listen_side,
            connect_side: args.connect_side,
            forwards: args
                .forwards
                .iter()
                .map(|value| parse_forward_spec(value))
                .collect::<anyhow::Result<Vec<_>>>()?,
        },
        ForwardPortsActionArgs::List(args) => ForwardPortsInput::List {
            listen_side: args.listen_side,
            connect_side: args.connect_side,
            forward_ids: args.forward_ids,
        },
        ForwardPortsActionArgs::Close(args) => ForwardPortsInput::Close {
            forward_ids: args.forward_ids,
        },
    })
}

impl ConnectionArgs {
    fn resolve(&self) -> anyhow::Result<Connection> {
        match (&self.broker_config, &self.broker_url) {
            (Some(config_path), None) => Ok(Connection::Config {
                config_path: config_path.clone(),
            }),
            (None, Some(url)) => Ok(Connection::StreamableHttp { url: url.clone() }),
            _ => unreachable!("clap should enforce exactly one connection mode"),
        }
    }
}

fn resolve_login_flag(login: bool, no_login: bool) -> Option<bool> {
    match (login, no_login) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

async fn load_required_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<String> {
    load_optional_text_input(inline, file)
        .await?
        .context("missing required text input")
}

async fn load_optional_text_input(
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

fn parse_transfer_endpoint(value: &str) -> anyhow::Result<TransferEndpoint> {
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

fn parse_forward_spec(value: &str) -> anyhow::Result<ForwardPortSpec> {
    let (protocol, endpoints) = value.split_once(':').with_context(|| {
        format!("invalid forward `{value}`; expected <protocol>:<listen>=<connect>")
    })?;
    let (listen_endpoint, connect_endpoint) = endpoints.split_once('=').with_context(|| {
        format!("invalid forward `{value}`; expected <protocol>:<listen>=<connect>")
    })?;
    let protocol = match protocol {
        "tcp" => ForwardPortProtocol::Tcp,
        "udp" => ForwardPortProtocol::Udp,
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

fn emit_response(response: &ToolResponse, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(response).context("serializing CLI response")?
        );
        return Ok(());
    }

    if response.is_error {
        if !response.text_output.is_empty() {
            eprintln!("{}", response.text_output);
        }
        return Ok(());
    }

    if !response.text_output.is_empty() {
        println!("{}", response.text_output);
    }

    Ok(())
}

fn emit_and_status(response: &ToolResponse, json: bool) -> anyhow::Result<i32> {
    emit_response(response, json)?;
    Ok(status_code(response))
}

fn emit_view_image_response(
    response: &ToolResponse,
    json: bool,
    output_path: Option<&Path>,
) -> anyhow::Result<()> {
    if json {
        return emit_response(response, true);
    }

    if response.is_error {
        return emit_response(response, false);
    }

    if let Some(path) = output_path {
        println!("Wrote image to {}", path.display());
        return Ok(());
    }

    if let Some(image_url) = response.first_image_url() {
        println!("{image_url}");
    }

    Ok(())
}

async fn write_image_output(response: &ToolResponse, out: &Path) -> anyhow::Result<()> {
    let image_url = response
        .first_image_url()
        .context("view_image response did not include an image payload")?;
    let bytes = decode_data_url(&image_url)?;
    tokio::fs::write(out, bytes)
        .await
        .with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

fn decode_data_url(image_url: &str) -> anyhow::Result<Vec<u8>> {
    let (metadata, payload) = image_url
        .split_once(',')
        .context("image payload was not a valid data URL")?;
    anyhow::ensure!(
        metadata.starts_with("data:") && metadata.ends_with(";base64"),
        "image payload was not a base64 data URL"
    );
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("decoding image data URL")
}

fn status_code(response: &ToolResponse) -> i32 {
    if response.is_error { 1 } else { 0 }
}
