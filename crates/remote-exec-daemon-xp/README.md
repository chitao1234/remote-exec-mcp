# remote-exec-daemon-xp

Standalone Windows XP daemon for `remote-exec-mcp`.

## Build

`make all`

Host-native verification:

- `make test-host-patch`
- `make test-host-transfer`
- `make test-host-config`
- `make test-host-http-request`
- `make check`

## Run

`build/remote-exec-daemon-xp.exe config/daemon-xp.example.ini`

Logs go to `stderr`. Set `REMOTE_EXEC_LOG=debug` to raise the level, or use a shared filter string such as `REMOTE_EXEC_LOG=warn,remote_exec_daemon_xp=debug`.

## Config

Example config:

```ini
target = builder-xp
listen_host = 0.0.0.0
listen_port = 8181
default_workdir = C:\work
```

## Limitations

- plain HTTP only in v1
- no PTY support
- no image support
- `transfer_files` supports regular files, directory trees, and broker-built multi-source bundles
- transfer payloads use GNU tar for both files and directories
- single-file transfers use the fixed archive entry `.remote-exec-file`
- transfer compression is not supported; XP only accepts uncompressed payloads
- unsupported archive entries remain rejected: symlinks, hard links, special files, sparse entries, and malformed paths
- `cmd.exe` only for shell execution
