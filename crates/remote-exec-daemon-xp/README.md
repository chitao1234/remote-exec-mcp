# remote-exec-daemon-xp

Standalone Windows XP daemon for `remote-exec-mcp`.

## Build

`make all`

Host-native verification:

- `make test-host-patch`
- `make test-host-transfer`
- `make check`

## Run

`build/remote-exec-daemon-xp.exe config/daemon-xp.example.ini`

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
- `transfer_files` supports regular files and directory trees
- directory transfers use GNU tar payloads for cross-target compatibility
- unsupported archive entries remain rejected: symlinks, hard links, special files, sparse entries, and malformed paths
- `cmd.exe` only for shell execution
