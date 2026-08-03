use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use remote_exec_host::{
    HostPortForwardLimits, HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig,
    build_runtime_state,
};
use remote_exec_proto::rpc::PatchApplyRequest;
use remote_exec_proto::transfer::TransferLimits;

const HELP: &str = "\
Apply a Codex-style patch read from standard input.

Usage: apply_patch [OPTIONS]

Options:
  -h, --help              Print this help text
      --help-file <PATH>  Print help text from PATH instead
";

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "apply_patch: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<OsString>) -> anyhow::Result<()> {
    if let Some(help_path) = parse_help_request(&args)? {
        let help = match help_path {
            Some(path) => std::fs::read_to_string(&path).map_err(|error| {
                anyhow::anyhow!("reading help file `{}`: {error}", path.display())
            })?,
            None => HELP.to_string(),
        };
        print!("{help}");
        return Ok(());
    }

    let mut patch = String::new();
    io::stdin()
        .read_to_string(&mut patch)
        .map_err(|error| anyhow::anyhow!("reading patch from standard input: {error}"))?;

    let state = Arc::new(build_runtime_state(local_config(env::current_dir()?))?);
    let response = remote_exec_host::patch::apply_patch_local(
        state,
        PatchApplyRequest {
            patch,
            workdir: None,
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.message))?;
    print!("{}", response.output);
    Ok(())
}

fn parse_help_request(args: &[OsString]) -> anyhow::Result<Option<Option<PathBuf>>> {
    match args {
        [] => Ok(None),
        [arg] if arg == "--help" || arg == "-h" => Ok(Some(None)),
        [arg, path] if arg == "--help-file" => Ok(Some(Some(PathBuf::from(path)))),
        [arg] if arg.to_string_lossy().starts_with("--help-file=") => {
            let value = arg.to_string_lossy();
            let Some(path) = value.strip_prefix("--help-file=") else {
                unreachable!("prefix was checked above");
            };
            if path.is_empty() {
                anyhow::bail!("--help-file requires a path");
            }
            Ok(Some(Some(PathBuf::from(path))))
        }
        [arg] if arg == "--help-file" => anyhow::bail!("--help-file requires a path"),
        _ => anyhow::bail!("unexpected argument; run `apply_patch --help` for usage"),
    }
}

fn local_config(default_workdir: PathBuf) -> HostRuntimeConfig {
    HostRuntimeConfig {
        target: "local".to_string(),
        default_workdir,
        windows_posix_root: None,
        sandbox: None,
        enable_transfer_compression: false,
        transfer_limits: TransferLimits::default(),
        max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
        allow_login_shell: false,
        pty: PtyMode::None,
        default_shell: None,
        yield_time: YieldTimeConfig::default(),
        port_forward_limits: HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: ProcessEnvironment::capture_current(),
    }
}
