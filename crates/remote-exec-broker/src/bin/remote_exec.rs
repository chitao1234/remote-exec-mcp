use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use remote_exec_broker::cli::{
    WriteStdinCliInput, build_apply_patch_input, build_edit_input, build_forward_ports_close_input,
    build_forward_ports_list_input, build_forward_ports_open_input, build_read_input,
    build_view_image_input, build_write_input, build_write_stdin_input, emit_response,
    emit_view_image_response, parse_transfer_endpoint, resolve_login_flag, write_image_output,
};
use remote_exec_broker::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::public::{ForwardPortsInput, ListTargetsInput, TransferDestinationMode};
use remote_exec_proto::transfer::{TransferOverwrite, TransferSymlinkMode};
use serde::Serialize;

const CLI_AFTER_HELP: &str = "\
Connection modes:
  --broker-config PATH   Load a broker config and call broker tools in-process.
  --broker-url URL       Connect to a running broker over streamable HTTP.

Exit codes:
  0 success, 2 usage/input, 3 config, 4 connection/transport, 5 tool error.

Examples:
  remote-exec --broker-config configs/broker.example.toml list-targets
  remote-exec --broker-url http://127.0.0.1:8787/mcp exec builder-a \"uname -a\"
";

const TRANSFER_AFTER_HELP: &str = "\
Endpoint format: <target>:<absolute-path>
Repeat --source to transfer multiple inputs.
For multi-source transfers, the destination path is treated as a directory root.
";

const WRITE_STDIN_AFTER_HELP: &str = "\
Positional forms:
  write-stdin <SESSION_ID> [CHARS]
  write-stdin <TARGET> <SESSION_ID> [CHARS]

The first argument is treated as a session id when it has the `sess_` form;
otherwise it is the target and the second argument is the session id.
Use --chars-file to read CHARS from a file or `-` instead of passing it inline.
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

    fn tool(error: anyhow::Error) -> Self {
        Self {
            exit_code: EXIT_TOOL,
            error,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "remote-exec", version = env!("REMOTE_EXEC_BUILD_VERSION"))]
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
    #[command(
        about = "List configured targets using cached broker metadata.",
        alias = "list_targets",
        visible_aliases = ["list", "ls"]
    )]
    ListTargets,
    #[command(
        name = "exec-command",
        visible_alias = "exec",
        alias = "exec_command",
        about = "Run a command on a configured target machine."
    )]
    Exec(ExecCommandArgs),
    #[command(
        about = "Write to or poll an existing exec session.",
        alias = "write_stdin",
        after_help = WRITE_STDIN_AFTER_HELP
    )]
    WriteStdin(WriteStdinArgs),
    #[command(
        about = "Apply a patch on a configured target machine.",
        alias = "apply_patch"
    )]
    ApplyPatch(ApplyPatchArgs),
    #[command(
        about = "Read an image from a configured target machine.",
        alias = "view_image"
    )]
    ViewImage(ViewImageArgs),
    #[command(
        about = "Transfer files or directory trees between local or configured targets.",
        alias = "transfer_files",
        visible_alias = "cp",
        after_help = TRANSFER_AFTER_HELP
    )]
    TransferFiles(TransferFilesArgs),
    #[command(about = "Read a text file from a configured target machine.")]
    Read(ReadArgs),
    #[command(about = "Overwrite or create a text file on a configured target machine.")]
    Write(WriteArgs),
    #[command(about = "Replace text in a file on a configured target machine.")]
    Edit(EditArgs),
    #[command(
        name = "forward-ports",
        alias = "forward_ports",
        about = "Open, list, or close broker-mediated TCP/UDP port forwards."
    )]
    ForwardPorts(ForwardPortsArgs),
}

#[derive(Args, Debug)]
struct ExecCommandArgs {
    #[arg(help = "Logical target name to run the command on.")]
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
struct WriteStdinArgs {
    #[arg(help = "Public session id, or the target when followed by a session id.")]
    first: String,

    #[arg(help = "Public session id or inline chars, after the first positional argument.")]
    second: Option<String>,

    #[arg(help = "Inline chars to write when a target and session id were provided.")]
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
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("patch_input").args(["input", "input_file"])
))]
struct ApplyPatchArgs {
    #[arg(help = "Logical target name to apply the patch on.")]
    target: String,

    #[arg(help = "Inline patch text to apply. Omit to read from stdin.")]
    input: Option<String>,

    #[arg(long, help = "Working directory used to resolve patch paths.")]
    workdir: Option<String>,

    #[arg(
        long,
        help = "Read patch text from a file, or use `-` to read from stdin."
    )]
    input_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ViewImageArgs {
    #[arg(help = "Logical target name to read the image from.")]
    target: String,

    #[arg(help = "Absolute image path on the selected target.")]
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
struct ReadArgs {
    #[arg(help = "Logical target name to read the file from.")]
    target: String,

    #[arg(help = "File path on the selected target.")]
    path: String,

    #[arg(long, help = "Zero-based line offset to start reading from.")]
    offset: Option<u64>,

    #[arg(long, help = "Maximum number of lines to return.")]
    limit: Option<u64>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("content_source").args(["content", "content_file"])
))]
struct WriteArgs {
    #[arg(help = "Logical target name to write the file on.")]
    target: String,

    #[arg(help = "File path on the selected target.")]
    path: String,

    #[arg(help = "Inline text content to write. Omit to read from stdin.")]
    content: Option<String>,

    #[arg(
        long,
        help = "Read content from a file, or use `-` to read from stdin."
    )]
    content_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("old_source")
        .required(true)
        .args(["old_string", "old_string_file"])
))]
#[command(group(
    ArgGroup::new("new_source")
        .args(["new_string", "new_string_file"])
))]
struct EditArgs {
    #[arg(help = "Logical target name to edit the file on.")]
    target: String,

    #[arg(help = "File path on the selected target.")]
    path: String,

    #[arg(help = "Inline text to replace. Omit when using --old-string-file.")]
    old_string: Option<String>,

    #[arg(help = "Inline replacement text. Omit to delete the matched text.")]
    new_string: Option<String>,

    #[arg(
        long,
        help = "Read the text to replace from a file, or use `-` to read from stdin."
    )]
    old_string_file: Option<PathBuf>,

    #[arg(
        long,
        help = "Read replacement text from a file, or use `-` to read from stdin."
    )]
    new_string_file: Option<PathBuf>,

    #[arg(
        long,
        default_value_t = false,
        help = "Replace every match instead of requiring a unique match."
    )]
    replace_all: bool,
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
        Command::Read(args) => run_read(client, args, json).await,
        Command::Write(args) => run_write(client, args, json).await,
        Command::Edit(args) => run_edit(client, args, json).await,
        Command::TransferFiles(args) => run_transfer_files(client, args, json).await,
        Command::ForwardPorts(args) => run_forward_ports(client, args, json).await,
    }
}

async fn run_list_targets(client: &RemoteExecClient, json: bool) -> CliResult<u8> {
    call_and_emit(client, "list_targets", &ListTargetsInput::default(), json).await
}

async fn run_exec(client: &RemoteExecClient, args: ExecCommandArgs, json: bool) -> CliResult<u8> {
    call_and_emit(client, "exec_command", &exec_command_input(args), json).await
}

async fn run_write_stdin(
    client: &RemoteExecClient,
    args: WriteStdinArgs,
    json: bool,
) -> CliResult<u8> {
    let input = write_stdin_input(args).await.map_err(CliError::usage)?;
    call_and_emit(client, "write_stdin", &input, json).await
}

async fn run_apply_patch(
    client: &RemoteExecClient,
    args: ApplyPatchArgs,
    json: bool,
) -> CliResult<u8> {
    let input = build_apply_patch_input(args.target, args.input, args.input_file, args.workdir)
        .await
        .map_err(CliError::usage)?;
    call_and_emit(client, "apply_patch", &input, json).await
}

async fn run_view_image(
    client: &RemoteExecClient,
    args: ViewImageArgs,
    json: bool,
) -> CliResult<u8> {
    let output_path = args.out;
    let input = build_view_image_input(args.target, args.path, args.workdir, args.detail);
    client
        .ensure_tool_available("view_image")
        .map_err(CliError::tool)?;
    let response = client
        .call_tool("view_image", &input)
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

async fn run_read(client: &RemoteExecClient, args: ReadArgs, json: bool) -> CliResult<u8> {
    let input = build_read_input(args.target, args.path, args.offset, args.limit);
    call_and_emit(client, "read", &input, json).await
}

async fn run_write(client: &RemoteExecClient, args: WriteArgs, json: bool) -> CliResult<u8> {
    let input = build_write_input(args.target, args.path, args.content, args.content_file)
        .await
        .map_err(CliError::usage)?;
    call_and_emit(client, "write", &input, json).await
}

async fn run_edit(client: &RemoteExecClient, args: EditArgs, json: bool) -> CliResult<u8> {
    let input = build_edit_input(
        args.target,
        args.path,
        args.old_string,
        args.old_string_file,
        args.new_string,
        args.new_string_file,
        args.replace_all,
    )
    .await
    .map_err(CliError::usage)?;
    call_and_emit(client, "edit", &input, json).await
}

async fn run_transfer_files(
    client: &RemoteExecClient,
    args: TransferFilesArgs,
    json: bool,
) -> CliResult<u8> {
    let input = transfer_files_input(args).map_err(CliError::usage)?;
    call_and_emit(client, "transfer_files", &input, json).await
}

async fn run_forward_ports(
    client: &RemoteExecClient,
    args: ForwardPortsArgs,
    json: bool,
) -> CliResult<u8> {
    let input = forward_ports_input(args).map_err(CliError::usage)?;
    call_and_emit(client, "forward_ports", &input, json).await
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
    build_write_stdin_input(WriteStdinCliInput {
        first: args.first,
        second: args.second,
        chars: args.chars,
        chars_file: args.chars_file,
        yield_time_ms: args.yield_time_ms,
        max_output_tokens: args.max_output_tokens,
        pty_rows: args.pty_rows,
        pty_cols: args.pty_cols,
    })
    .await
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

async fn call_and_emit<T>(
    client: &RemoteExecClient,
    tool: &str,
    input: &T,
    json: bool,
) -> CliResult<u8>
where
    T: Serialize + ?Sized,
{
    client.ensure_tool_available(tool).map_err(CliError::tool)?;
    let response = client
        .call_tool(tool, input)
        .await
        .map_err(CliError::connection)?;
    emit_and_status(&response, json)
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
