use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use remote_exec_broker::client::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::public::{
    ApplyPatchInput, ExecCommandInput, ListTargetsInput, TransferEndpoint, TransferFilesInput,
    TransferOverwrite, ViewImageInput, WriteStdinInput,
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

    let exit_code = match cli.command {
        Command::ListTargets => {
            let response = client
                .call_tool("list_targets", &ListTargetsInput::default())
                .await?;
            emit_response(&response, cli.json)?;
            status_code(&response)
        }
        Command::Exec(args) => {
            let response = client
                .call_tool(
                    "exec_command",
                    &ExecCommandInput {
                        target: args.target,
                        cmd: args.cmd,
                        workdir: args.workdir,
                        shell: args.shell,
                        tty: args.tty,
                        yield_time_ms: args.yield_time_ms,
                        max_output_tokens: args.max_output_tokens,
                        login: resolve_login_flag(args.login, args.no_login),
                    },
                )
                .await?;
            emit_response(&response, cli.json)?;
            status_code(&response)
        }
        Command::WriteStdin(args) => {
            let response = client
                .call_tool(
                    "write_stdin",
                    &WriteStdinInput {
                        session_id: args.session_id,
                        chars: load_optional_text_input(args.chars, args.chars_file).await?,
                        yield_time_ms: args.yield_time_ms,
                        max_output_tokens: args.max_output_tokens,
                        target: args.target,
                    },
                )
                .await?;
            emit_response(&response, cli.json)?;
            status_code(&response)
        }
        Command::ApplyPatch(args) => {
            let response = client
                .call_tool(
                    "apply_patch",
                    &ApplyPatchInput {
                        target: args.target,
                        input: load_required_text_input(args.input, args.input_file).await?,
                        workdir: args.workdir,
                    },
                )
                .await?;
            emit_response(&response, cli.json)?;
            status_code(&response)
        }
        Command::ViewImage(args) => {
            let response = client
                .call_tool(
                    "view_image",
                    &ViewImageInput {
                        target: args.target,
                        path: args.path,
                        workdir: args.workdir,
                        detail: args.detail,
                    },
                )
                .await?;
            if !response.is_error
                && let Some(out) = &args.out
            {
                write_image_output(&response, out).await?;
            }
            emit_view_image_response(&response, cli.json, args.out.as_deref())?;
            status_code(&response)
        }
        Command::TransferFiles(args) => {
            let endpoints = args
                .sources
                .iter()
                .map(|endpoint| parse_transfer_endpoint(endpoint))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let response = client
                .call_tool(
                    "transfer_files",
                    &TransferFilesInput {
                        source: (endpoints.len() == 1).then(|| endpoints[0].clone()),
                        sources: if endpoints.len() == 1 {
                            Vec::new()
                        } else {
                            endpoints
                        },
                        destination: parse_transfer_endpoint(&args.destination)?,
                        overwrite: args.overwrite.into(),
                        create_parent: args.create_parent,
                    },
                )
                .await?;
            emit_response(&response, cli.json)?;
            status_code(&response)
        }
    };

    std::process::exit(exit_code);
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
