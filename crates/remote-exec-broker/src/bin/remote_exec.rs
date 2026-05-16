use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use remote_exec_broker::cli::{
    build_apply_patch_input, build_forward_ports_close_input, build_forward_ports_list_input,
    build_forward_ports_open_input, build_view_image_input, emit_response,
    emit_view_image_response, load_optional_text_input, parse_transfer_endpoint,
    resolve_login_flag, write_image_output, write_stdin_pty_size,
};
use remote_exec_broker::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::public::{
    ForwardPortsInput, ListTargetsInput, TransferDestinationMode, TransferOverwrite,
    TransferSymlinkMode,
};

const CLI_AFTER_HELP: &str = "\
Connection modes:
  --broker-config PATH   Load a broker config and call broker tools in-process.
  --broker-url URL       Connect to a running broker over streamable HTTP.

Exit codes:
  0 success, 2 usage/input, 3 config, 4 connection/transport, 5 tool error.

Examples:
  remote-exec --broker-config configs/broker.example.toml list-targets
  remote-exec --broker-url http://127.0.0.1:8787/mcp exec --target builder-a \"uname -a\"
";

const TRANSFER_AFTER_HELP: &str = "\
Endpoint format: <target>:<absolute-path>
Repeat --source to transfer multiple inputs.
For multi-source transfers, the destination path is treated as a directory root.
";

const EXIT_SUCCESS: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_CONFIG: u8 = 3;
const EXIT_CONNECTION: u8 = 4;
const EXIT_TOOL: u8 = 5;

type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
struct CliError {
    exit_code: u8,
    error: anyhow::Error,
}

impl CliError {
    fn usage(error: anyhow::Error) -> Self {
        Self {
            exit_code: EXIT_USAGE,
            error,
        }
    }

    fn config(error: anyhow::Error) -> Self {
        Self {
            exit_code: EXIT_CONFIG,
            error,
        }
    }

    fn connection(error: anyhow::Error) -> Self {
        Self {
            exit_code: EXIT_CONNECTION,
            error,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "remote-exec")]
#[command(
    about = "CLI client for a remote-exec-mcp broker",
    long_about = "Connect to a remote-exec broker over an in-process config or streamable HTTP and call its public remote execution tools.",
    after_help = CLI_AFTER_HELP
)]
struct Cli {
    #[command(flatten)]
    connection: ConnectionArgs,

    #[arg(
        long,
        default_value_t = false,
        help = "Print the normalized tool response object as JSON."
    )]
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
    #[arg(
        long,
        help = "Load this broker config and call broker tools in-process."
    )]
    broker_config: Option<PathBuf>,

    #[arg(long, help = "Connect to a running broker over streamable HTTP.")]
    broker_url: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "List configured targets using cached broker metadata.")]
    ListTargets,
    #[command(
        name = "exec-command",
        visible_alias = "exec",
        about = "Run a command on a configured target machine."
    )]
    Exec(ExecCommandArgs),
    #[command(about = "Write to or poll an existing exec session.")]
    WriteStdin(WriteStdinArgs),
    #[command(about = "Apply a patch on a configured target machine.")]
    ApplyPatch(ApplyPatchArgs),
    #[command(about = "Read an image from a configured target machine.")]
    ViewImage(ViewImageArgs),
    #[command(
        about = "Transfer files or directory trees between broker-local and configured targets.",
        after_help = TRANSFER_AFTER_HELP
    )]
    TransferFiles(TransferFilesArgs),
    #[command(
        name = "forward-ports",
        about = "Open, list, or close broker-mediated TCP/UDP port forwards."
    )]
    ForwardPorts(ForwardPortsArgs),
}

#[derive(Args, Debug)]
struct ExecCommandArgs {
    #[arg(long, help = "Logical target name to run the command on.")]
    target: String,

    #[arg(help = "Shell command text to execute.")]
    cmd: String,

    #[arg(long, help = "Working directory to start the command in.")]
    workdir: Option<String>,

    #[arg(long, help = "Override the shell used to launch the command.")]
    shell: Option<String>,

    #[arg(long, default_value_t = false, help = "Request a PTY session.")]
    tty: bool,

    #[arg(
        long,
        help = "Milliseconds to wait for initial output before returning."
    )]
    yield_time_ms: Option<u64>,

    #[arg(long, help = "Maximum number of output tokens to return.")]
    max_output_tokens: Option<u32>,

    #[arg(
        long,
        default_value_t = false,
        overrides_with = "no_login",
        help = "Force login shell semantics."
    )]
    login: bool,

    #[arg(
        long,
        default_value_t = false,
        overrides_with = "login",
        help = "Disable login shell semantics."
    )]
    no_login: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("input").args(["chars", "chars_file"])))]
struct WriteStdinArgs {
    #[arg(long, help = "Opaque public session id returned by exec-command.")]
    session_id: String,

    #[arg(long, help = "Inline text to write to the session. Omit to poll only.")]
    chars: Option<String>,

    #[arg(
        long,
        help = "Read stdin payload from a file, or use `-` to read from stdin."
    )]
    chars_file: Option<PathBuf>,

    #[arg(long, help = "Milliseconds to wait for output before returning.")]
    yield_time_ms: Option<u64>,

    #[arg(long, help = "Maximum number of output tokens to return.")]
    max_output_tokens: Option<u32>,

    #[arg(
        long,
        help = "Resize PTY rows for this live session; requires --pty-cols."
    )]
    pty_rows: Option<u16>,

    #[arg(
        long,
        help = "Resize PTY columns for this live session; requires --pty-rows."
    )]
    pty_cols: Option<u16>,

    #[arg(long, help = "Optional target check for the session.")]
    target: Option<String>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("patch_input")
        .required(true)
        .args(["input", "input_file"])
))]
struct ApplyPatchArgs {
    #[arg(long, help = "Logical target name to apply the patch on.")]
    target: String,

    #[arg(long, help = "Working directory used to resolve patch paths.")]
    workdir: Option<String>,

    #[arg(long, help = "Inline patch text to apply.")]
    input: Option<String>,

    #[arg(
        long,
        help = "Read patch text from a file, or use `-` to read from stdin."
    )]
    input_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ViewImageArgs {
    #[arg(long, help = "Logical target name to read the image from.")]
    target: String,

    #[arg(long, help = "Absolute image path on the selected target.")]
    path: String,

    #[arg(
        long,
        help = "Working directory used to resolve relative paths if supported."
    )]
    workdir: Option<String>,

    #[arg(
        long,
        help = "Image detail level. Use `original` for full fidelity when supported."
    )]
    detail: Option<String>,

    #[arg(
        long,
        help = "Write the decoded image bytes to this local output path."
    )]
    out: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct TransferFilesArgs {
    #[arg(
        long = "source",
        value_name = "TARGET:PATH",
        required = true,
        help = "Source endpoint in <target>:<absolute-path> format. Repeat --source to transfer multiple inputs."
    )]
    sources: Vec<String>,

    #[arg(
        long,
        value_name = "TARGET:PATH",
        help = "Destination endpoint in <target>:<absolute-path> format."
    )]
    destination: String,

    #[arg(
        long = "exclude",
        help = "Glob pattern to exclude during export. Repeat for multiple patterns."
    )]
    exclude: Vec<String>,

    #[arg(
        long,
        value_enum,
        default_value_t = CliTransferOverwrite::Merge,
        help = "How to handle an existing destination."
    )]
    overwrite: CliTransferOverwrite,

    #[arg(
        long,
        value_enum,
        default_value_t = CliTransferDestinationMode::Auto,
        help = "How to resolve the destination path."
    )]
    destination_mode: CliTransferDestinationMode,

    #[arg(
        long,
        value_enum,
        default_value_t = CliTransferSymlinkMode::Preserve,
        help = "How to handle symlinks while exporting."
    )]
    symlink_mode: CliTransferSymlinkMode,

    #[arg(
        long,
        default_value_t = false,
        help = "Create missing parent directories for the destination path."
    )]
    create_parent: bool,
}

#[derive(Args, Debug)]
struct ForwardPortsArgs {
    #[command(subcommand)]
    action: ForwardPortsActionArgs,
}

#[derive(Subcommand, Debug)]
enum ForwardPortsActionArgs {
    #[command(about = "Open one or more broker-mediated TCP/UDP port forwards.")]
    Open(ForwardPortsOpenArgs),
    #[command(about = "List open port forwards, optionally filtered by side or id.")]
    List(ForwardPortsListArgs),
    #[command(about = "Close one or more existing port forwards by id.")]
    Close(ForwardPortsCloseArgs),
}

#[derive(Args, Debug)]
struct ForwardPortsOpenArgs {
    #[arg(long, help = "Side that binds the listen endpoint.")]
    listen_side: String,

    #[arg(long, help = "Side that connects to the destination endpoint.")]
    connect_side: String,

    #[arg(
        long = "forward",
        required = true,
        help = "Forward spec in the form <protocol>:<listen>=<connect>, for example tcp:127.0.0.1:0=127.0.0.1:5432"
    )]
    forwards: Vec<String>,
}

#[derive(Args, Debug)]
struct ForwardPortsListArgs {
    #[arg(long, help = "Filter by listen side.")]
    listen_side: Option<String>,

    #[arg(long, help = "Filter by connect side.")]
    connect_side: Option<String>,

    #[arg(long = "forward-id", help = "Filter by forward id. Repeatable.")]
    forward_ids: Vec<String>,
}

#[derive(Args, Debug)]
struct ForwardPortsCloseArgs {
    #[arg(
        long = "forward-id",
        required = true,
        help = "Forward id to close. Repeatable."
    )]
    forward_ids: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliTransferOverwrite {
    Fail,
    Merge,
    Replace,
}

impl From<CliTransferOverwrite> for TransferOverwrite {
    fn from(value: CliTransferOverwrite) -> Self {
        match value {
            CliTransferOverwrite::Fail => Self::Fail,
            CliTransferOverwrite::Merge => Self::Merge,
            CliTransferOverwrite::Replace => Self::Replace,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliTransferDestinationMode {
    Auto,
    Exact,
    IntoDirectory,
}

impl From<CliTransferDestinationMode> for TransferDestinationMode {
    fn from(value: CliTransferDestinationMode) -> Self {
        match value {
            CliTransferDestinationMode::Auto => Self::Auto,
            CliTransferDestinationMode::Exact => Self::Exact,
            CliTransferDestinationMode::IntoDirectory => Self::IntoDirectory,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliTransferSymlinkMode {
    Preserve,
    Follow,
    Skip,
}

impl From<CliTransferSymlinkMode> for TransferSymlinkMode {
    fn from(value: CliTransferSymlinkMode) -> Self {
        match value {
            CliTransferSymlinkMode::Preserve => Self::Preserve,
            CliTransferSymlinkMode::Follow => Self::Follow,
            CliTransferSymlinkMode::Skip => Self::Skip,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let exit_code = match try_main().await {
        Ok(exit_code) => exit_code,
        Err(err) => {
            eprintln!("{:#}", err.error);
            err.exit_code
        }
    };
    ExitCode::from(exit_code)
}

async fn try_main() -> CliResult<u8> {
    let cli = Cli::parse();
    let connection = cli.connection.resolve().map_err(CliError::usage)?;
    let client = RemoteExecClient::connect(connection.clone())
        .await
        .map_err(|err| match connection {
            Connection::Config { .. } => CliError::config(err),
            Connection::StreamableHttp { .. } => CliError::connection(err),
        })?;
    let exit_code = run_command(&client, cli.command, cli.json).await?;
    Ok(exit_code)
}

async fn run_command(client: &RemoteExecClient, command: Command, json: bool) -> CliResult<u8> {
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

async fn run_list_targets(client: &RemoteExecClient, json: bool) -> CliResult<u8> {
    let response = client
        .call_tool("list_targets", &ListTargetsInput::default())
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

async fn run_exec(client: &RemoteExecClient, args: ExecCommandArgs, json: bool) -> CliResult<u8> {
    let response = client
        .call_tool("exec_command", &exec_command_input(args))
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

async fn run_write_stdin(
    client: &RemoteExecClient,
    args: WriteStdinArgs,
    json: bool,
) -> CliResult<u8> {
    let input = write_stdin_input(args).await.map_err(CliError::usage)?;
    let response = client
        .call_tool("write_stdin", &input)
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

async fn run_apply_patch(
    client: &RemoteExecClient,
    args: ApplyPatchArgs,
    json: bool,
) -> CliResult<u8> {
    let input = apply_patch_input(args).await.map_err(CliError::usage)?;
    let response = client
        .call_tool("apply_patch", &input)
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

async fn run_view_image(
    client: &RemoteExecClient,
    args: ViewImageArgs,
    json: bool,
) -> CliResult<u8> {
    let output_path = args.out.clone();
    let response = client
        .call_tool("view_image", &view_image_input(args))
        .await
        .map_err(CliError::connection)?;
    if !response.is_error {
        if let Some(out) = &output_path {
            write_image_output(&response, out)
                .await
                .map_err(CliError::usage)?;
        }
    }
    emit_view_image_response(&response, json, output_path.as_deref()).map_err(CliError::usage)?;
    Ok(status_code(&response))
}

async fn run_transfer_files(
    client: &RemoteExecClient,
    args: TransferFilesArgs,
    json: bool,
) -> CliResult<u8> {
    let input = transfer_files_input(args).map_err(CliError::usage)?;
    let response = client
        .call_tool("transfer_files", &input)
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

async fn run_forward_ports(
    client: &RemoteExecClient,
    args: ForwardPortsArgs,
    json: bool,
) -> CliResult<u8> {
    let input = forward_ports_input(args).map_err(CliError::usage)?;
    let response = client
        .call_tool("forward_ports", &input)
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
}

fn exec_command_input(args: ExecCommandArgs) -> remote_exec_proto::public::ExecCommandInput {
    remote_exec_proto::public::ExecCommandInput {
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

async fn write_stdin_input(
    args: WriteStdinArgs,
) -> anyhow::Result<remote_exec_proto::public::WriteStdinInput> {
    Ok(remote_exec_proto::public::WriteStdinInput {
        session_id: args.session_id,
        chars: load_optional_text_input(args.chars, args.chars_file).await?,
        yield_time_ms: args.yield_time_ms,
        max_output_tokens: args.max_output_tokens,
        pty_size: write_stdin_pty_size(args.pty_rows, args.pty_cols)?,
        target: args.target,
    })
}

async fn apply_patch_input(
    args: ApplyPatchArgs,
) -> anyhow::Result<remote_exec_proto::public::ApplyPatchInput> {
    build_apply_patch_input(args.target, args.input, args.input_file, args.workdir).await
}

fn view_image_input(args: ViewImageArgs) -> remote_exec_proto::public::ViewImageInput {
    build_view_image_input(args.target, args.path, args.workdir, args.detail)
}

fn transfer_files_input(
    args: TransferFilesArgs,
) -> anyhow::Result<remote_exec_proto::public::TransferFilesInput> {
    let endpoints = args
        .sources
        .iter()
        .map(|endpoint| parse_transfer_endpoint(endpoint))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(remote_exec_proto::public::TransferFilesInput {
        source: None,
        sources: endpoints,
        destination: parse_transfer_endpoint(&args.destination)?,
        exclude: args.exclude,
        overwrite: args.overwrite.into(),
        destination_mode: args.destination_mode.into(),
        symlink_mode: args.symlink_mode.into(),
        create_parent: args.create_parent,
    })
}

fn forward_ports_input(args: ForwardPortsArgs) -> anyhow::Result<ForwardPortsInput> {
    Ok(match args.action {
        ForwardPortsActionArgs::Open(args) => {
            build_forward_ports_open_input(args.listen_side, args.connect_side, &args.forwards)?
        }
        ForwardPortsActionArgs::List(args) => {
            build_forward_ports_list_input(args.listen_side, args.connect_side, args.forward_ids)
        }
        ForwardPortsActionArgs::Close(args) => build_forward_ports_close_input(args.forward_ids),
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

fn emit_and_status(response: &ToolResponse, json: bool) -> CliResult<u8> {
    emit_response(response, json).map_err(CliError::usage)?;
    Ok(status_code(response))
}

fn status_code(response: &ToolResponse) -> u8 {
    if response.is_error {
        EXIT_TOOL
    } else {
        EXIT_SUCCESS
    }
}
