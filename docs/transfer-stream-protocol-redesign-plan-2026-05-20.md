# Transfer Stream Protocol Redesign Plan

Date: 2026-05-20

Status: planning

Scope:

- `crates/remote-exec-proto/`
- `crates/remote-exec-broker/`
- `crates/remote-exec-host/`
- `crates/remote-exec-daemon/`
- `crates/remote-exec-daemon-cpp/`

Related:

- `docs/cpp-daemon-architecture-redesign-plan-2026-05-19.md`

## Purpose

This document replaces the previous transfer-stream compatibility idea with a
single new broker-daemon transfer protocol.

The current transfer export wire format is raw archive bytes over HTTP. That is
not enough for true streaming because success is committed when HTTP headers are
sent, but archive generation can still fail later. Rust currently streams and
can only produce a truncated archive on late failure. The C++ daemon currently
buffers before sending headers to avoid that correctness bug, but buffering is
not true streaming.

The new design must address both problems:

- Rust must stop exposing late archive-generation failures as truncated raw
  archive streams.
- C++ must be able to stream export data without buffering the full archive in
  daemon memory.

## Compatibility Decision

Drop older transfer stream support.

There will be no long-term v1 raw-archive fallback in the broker, Rust daemon,
or C++ daemon. The new framed transfer stream becomes the only broker-daemon
transfer export/import body format once the plan is implemented.

The existing route paths can remain unchanged:

- `/v1/transfer/export`
- `/v1/transfer/import`

The `v1` route prefix is the existing daemon RPC namespace, not the archive body
protocol version. The route names can stay stable while the transfer body format
changes.

The broker should reject or mark unavailable any daemon that does not report the
new transfer protocol. The daemons should reject missing or wrong transfer
protocol headers with a normal JSON RPC error. Do not add downgrade negotiation
unless a later task explicitly restores old-daemon support.

## Current Protocol Problem

The old export response shape is:

```text
HTTP/1.1 200 OK
Content-Type: application/octet-stream
Transfer-Encoding: chunked
x-remote-exec-source-type: file|directory|multiple
x-remote-exec-compression: none|zstd

<raw tar or compressed tar bytes>
```

That has no daemon-level terminal success marker and no daemon-level late error
marker. HTTP chunking only says how bytes are transported; it does not say
whether the transfer completed correctly.

This creates bad outcomes:

- if export fails after headers, the peer sees EOF or a truncated archive
  instead of a typed daemon error;
- if the broker streams that body directly into another daemon import, the
  destination can receive partial data before the source failure is known;
- if C++ avoids that by buffering the entire archive before headers, memory
  pressure scales with archive size and the implementation is not truly
  streaming.

## Target Protocol

The transfer body is a binary frame stream. Archive bytes are carried as data
frames. Transfer success or failure is committed by an explicit terminal frame.

### Constants

Add shared constants to `remote-exec-proto` and mirror them in the C++ daemon
contract:

```text
TRANSFER_STREAM_PROTOCOL_VERSION = 2
TRANSFER_STREAM_VERSION_HEADER = x-remote-exec-transfer-stream-version
TRANSFER_STREAM_CONTENT_TYPE = application/vnd.remote-exec.transfer-stream.v2
TRANSFER_STREAM_PREFACE = "REXFER2\n"
```

The protocol is versioned so incompatible future changes have a clear guard, but
only version `2` is supported after this migration.

### Required Headers

Export request:

```text
POST /v1/transfer/export
Content-Type: application/json
x-remote-exec-transfer-stream-version: 2
```

Export response on success:

```text
HTTP/1.1 200 OK
Content-Type: application/vnd.remote-exec.transfer-stream.v2
Transfer-Encoding: chunked
x-remote-exec-transfer-stream-version: 2
x-remote-exec-source-type: file|directory|multiple
x-remote-exec-compression: none|zstd
```

Import request:

```text
POST /v1/transfer/import
Content-Type: application/vnd.remote-exec.transfer-stream.v2
Transfer-Encoding: chunked
x-remote-exec-transfer-stream-version: 2
x-remote-exec-destination-path: <base64 path>
x-remote-exec-overwrite: fail|merge|replace
x-remote-exec-create-parent: true|false
x-remote-exec-source-type: file|directory|multiple
x-remote-exec-compression: none|zstd
x-remote-exec-symlink-mode: preserve|follow|skip
```

Import response remains JSON:

```json
{
  "source_type": "directory",
  "bytes_copied": 123,
  "files_copied": 4,
  "directories_copied": 2,
  "replaced": false,
  "warnings": []
}
```

Pre-stream failures remain normal JSON RPC errors. For example a missing source
path is still a `400` response with `transfer_source_missing`.

### Frame Format

All integer fields are big-endian.

```text
stream preface:
  8 bytes  "REXFER2\n"

frame:
  type      1 byte
  flags     1 byte, must be 0
  reserved  2 bytes, must be 0
  length    8 bytes unsigned
  payload   length bytes
```

Frame types:

```text
0x01 DATA
0x02 COMPLETE
0x03 ERROR
```

Rules:

- `DATA` payload is archive bytes after the selected compression transform.
- `COMPLETE` is terminal success.
- `ERROR` is terminal failure.
- Exactly one terminal frame is required.
- EOF before a terminal frame is a transfer failure.
- No frames may appear after a terminal frame.
- Unknown frame types are transfer failures.
- Non-zero flags or reserved fields are transfer failures.
- `DATA` frame payloads must be bounded. Start with 64 KiB.
- `COMPLETE` and `ERROR` payloads must be separately bounded. Start with
  64 KiB.

`COMPLETE` payload:

```json
{"archive_bytes":123456}
```

`ERROR` payload uses the existing RPC error shape:

```json
{"code":"transfer_failed","message":"source file changed while exporting"}
```

The broker and daemons must not treat the stream as successful until a valid
`COMPLETE` frame is read.

## Export Semantics

Export has two phases.

### Preflight Before Response Commitment

Before sending `200 OK`, the daemon should:

- parse and validate the request;
- validate the required transfer stream version header;
- validate compression support;
- resolve the source path;
- compile exclude patterns;
- determine source type;
- authorize the requested source path;
- recursively walk directory exports enough to authorize all materialized child
  source paths;
- build an export plan with enough metadata to write archive entries.

Failures in this phase produce ordinary JSON RPC errors before any stream
headers are sent.

This is important for sandbox and validation failures: a recursive deny should
not be reported as a late stream error when it can be detected before response
commitment.

### Streaming After Response Commitment

After preflight succeeds, the daemon sends the framed response and streams
archive bytes as `DATA` frames.

Late failures become terminal `ERROR` frames:

- source file removed after planning;
- source file read failure;
- source file type changes after planning;
- compression failure;
- archive writer failure;
- other unavoidable race or I/O failure.

Successful completion sends a terminal `COMPLETE` frame.

Peer disconnect during send is logged locally and does not require an `ERROR`
frame because there is no peer left to receive it.

## Import Semantics

Import must not commit filesystem changes until both conditions are true:

- archive parsing and validation succeeded;
- the framed stream reached terminal `COMPLETE`.

The import path should be:

1. Parse framed `DATA` payloads as archive bytes.
2. Treat stream `ERROR` as transfer failure.
3. Treat EOF before `COMPLETE` as transfer failure.
4. Validate strict archive terminator.
5. Build an import plan containing all materialized paths and temp commit paths.
6. Authorize all materialized destination paths and symlink targets.
7. Require terminal `COMPLETE`.
8. Commit the plan.

If any step before commit fails, no destination output should remain. If any
commit step fails, staged temp paths should be cleaned up as far as possible and
the route should report failure.

This means the C++ import implementation should not keep the current
`parse-and-commit` shape for framed imports. It needs an explicit
`plan -> require stream complete -> commit` boundary.

The Rust import implementation needs the same staged-plan behavior. A direct
streaming extractor that writes destination files before the transfer terminal
frame is not acceptable for the new protocol.

## Broker Semantics

The broker is the only client expected to speak this daemon-private protocol.

Broker export handling:

- always send `x-remote-exec-transfer-stream-version: 2`;
- require response `Content-Type` and version headers for stream responses;
- decode the preface and frames;
- write only `DATA` payloads to archive consumers;
- treat stream `ERROR` as the daemon RPC error it carries;
- treat EOF before terminal frame as a transfer failure;
- expose success only after `COMPLETE`.

Broker remote export to temp file:

- write data to a temporary path;
- flush/sync if needed by existing local policy;
- keep the temp path only after `COMPLETE`;
- delete the temp path on `ERROR`, malformed stream, or EOF before terminal.

Broker single-source transfer:

- remote or local source export should become a stream object whose read side
  can surface terminal-frame failure;
- destination import must not be treated as successful unless the source stream
  completed and the destination import returned success.

Broker remote-to-remote transfer:

- first implementation may spool the source stream to a broker temp archive and
  then import that archive. This is simpler and preserves correctness.
- direct streaming from source daemon to destination daemon is allowed only
  after both Rust and C++ import implementations prove that `ERROR` and
  EOF-before-`COMPLETE` leave no committed output.

Since older support is dropped, there should be no v1 fallback path in these
operations after the migration is complete.

## Rust Implementation Plan

### `remote-exec-proto`

Add a transfer stream module with:

- protocol constants;
- frame type enum;
- frame header encode/decode helpers;
- terminal payload structs;
- tests for frame round trips, malformed headers, oversized payloads, and
  terminal-state rules.

Keep this module independent from async runtime details so the broker, Rust
daemon, and tests can share the same wire definitions.

### `remote-exec-host`

Refactor transfer export into a plan-driven writer:

- create an export plan before response commitment;
- recursively authorize materialized source children during planning;
- keep path metadata needed to write tar entries;
- stream archive bytes into a generic writer;
- return late writer/source errors to the caller so the daemon can emit a
  terminal `ERROR` frame.

Refactor import into staged planning:

- decode archive bytes from a reader into an import plan;
- avoid writing final destination paths while parsing;
- use temp paths for file bodies where feasible;
- authorize every materialized destination path before commit;
- expose commit as a separate step after the caller has observed stream
  `COMPLETE`.

### `remote-exec-daemon`

Export route:

- require transfer stream version `2`;
- preflight and build export plan before response headers;
- return JSON RPC errors for preflight failures;
- stream framed data through `Body::from_stream`;
- send terminal `COMPLETE` on success;
- send terminal `ERROR` on late failure.

Import route:

- require transfer stream version `2`;
- wrap request body in a frame decoder;
- parse/stage import plan from `DATA`;
- require terminal `COMPLETE`;
- commit plan;
- return JSON import summary.

Target info:

- report `transfer_stream_protocol_version: 2`;
- broker startup should treat missing or wrong value as unsupported for transfer
  operations.

## C++ Implementation Plan

Preserve:

- C++11;
- thread-based concurrency;
- no `openat`;
- GNU make, BSD make, and NMAKE entry points;
- Windows XP-compatible build path.

### Protocol Constants And Codec

Add C++ constants mirroring `remote-exec-proto`:

- `TRANSFER_STREAM_PROTOCOL_VERSION`;
- `TRANSFER_STREAM_VERSION_HEADER`;
- `TRANSFER_STREAM_CONTENT_TYPE`;
- frame type values and limits.

Add a small C++ frame codec:

- encode frame headers;
- decode frame headers;
- validate preface;
- enforce frame length limits;
- write `DATA`, `COMPLETE`, and `ERROR` frames.

Keep raw socket send/receive behind existing platform/http helpers.

### Export Planning

Replace the current "write archive to string, then send" export route with:

- request validation;
- export source type detection;
- exclude matcher validation;
- recursive export plan creation;
- recursive sandbox authorization before response commitment;
- framed streaming archive sink after response commitment.

The export plan should identify entries without reading full file bodies into
memory. File bodies should still be read during archive streaming.

The framed archive sink should:

- buffer no more than one bounded frame payload at a time;
- write archive bytes as `DATA` frames;
- count archive bytes for the `COMPLETE` payload;
- propagate send failures distinctly from transfer failures.

Late failures should be converted into a terminal `ERROR` frame where possible.

### Import Planning

Refactor current import APIs so framed import can do:

```text
read framed DATA -> parse archive -> build plan -> require COMPLETE -> commit
```

Do not commit before the terminal frame is observed.

This likely means splitting current C++ import internals into public or internal
steps:

- parse file archive into `ImportPlan`;
- parse directory/multiple archive into `ImportPlan`;
- prepare commit paths and authorize them;
- require framed stream complete;
- execute file import plan;
- execute directory import plan.

The current C++ import plan already handles strict terminators, temp paths, and
recursive destination authorization. The missing boundary is making terminal
stream completion happen before commit.

### HTTP/RPC Boundary

Move route-specific chunked/framed response mechanics out of
`server_route_transfer.cpp` as part of this work.

The route should:

- decode request;
- build preflight plan;
- ask a transfer-stream writer/reader to handle frame I/O;
- translate preflight failures to JSON RPC errors;
- log terminal stream failures.

## Contract And Schema Changes

Update together:

- `crates/remote-exec-proto/src/rpc/transfer*`;
- Rust daemon transfer routes;
- Rust host transfer implementation;
- broker daemon client transfer codec;
- broker transfer operations;
- C++ `server_contract`;
- C++ transfer route and transfer ops;
- target-info structs and parsers;
- README/config/skill docs if user-facing transfer behavior changes.

The public MCP `transfer_files` tool should not expose frame details. The public
contract remains "copy these files between endpoints." The protocol is
daemon-private.

## Testing Plan

### Shared Protocol Tests

- frame codec round trips `DATA`, `COMPLETE`, and `ERROR`;
- decoder rejects bad preface;
- decoder rejects non-zero flags;
- decoder rejects non-zero reserved bytes;
- decoder rejects oversized payloads;
- decoder rejects unknown frame types;
- decoder rejects EOF before terminal frame;
- decoder rejects frames after terminal frame.

### Rust Tests

- export file returns framed stream and terminal `COMPLETE`;
- export directory returns framed stream and terminal `COMPLETE`;
- export preflight missing source returns JSON `transfer_source_missing`;
- export recursive sandbox deny returns JSON `sandbox_denied` before stream
  headers;
- forced late export failure returns terminal `ERROR`;
- broker decodes terminal `ERROR` as transfer failure;
- broker removes temp archive on terminal `ERROR`;
- broker removes temp archive on EOF before terminal frame;
- import framed stream commits only after `COMPLETE`;
- import framed `ERROR` leaves no destination output;
- import EOF before `COMPLETE` leaves no destination output.

### C++ Tests

- C++ frame codec tests for the same malformed cases;
- streaming export file sends `DATA` and `COMPLETE`;
- streaming export directory sends `DATA` and `COMPLETE`;
- export recursive sandbox deny has no framed response body and returns JSON
  error;
- forced late export failure sends terminal `ERROR`;
- framed import `COMPLETE` commits output;
- framed import `ERROR` before commit leaves no output;
- framed import EOF before `COMPLETE` leaves no output;
- framed import terminal `ERROR` after archive terminator still prevents commit.

### Integration Tests

- broker transfer from Rust daemon to local;
- broker transfer from C++ daemon to local;
- broker transfer from local to Rust daemon;
- broker transfer from local to C++ daemon;
- broker transfer from Rust daemon to C++ daemon;
- broker transfer from C++ daemon to Rust daemon;
- multi-source transfer still works through broker temp archives.

### Build Gates

Rust:

- `cargo test -p remote-exec-proto`
- `cargo test -p remote-exec-daemon --test transfer_rpc`
- `cargo test -p remote-exec-broker --test mcp_transfer`
- `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp` if C++ daemon
  startup metadata changes affect those fixtures

C++:

- `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- `bmake -C crates/remote-exec-daemon-cpp test-host-transfer`
- `bmake -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- `make -C crates/remote-exec-daemon-cpp check-posix`
- `bmake -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp check-windows-xp`

## Implementation Order

Use small commits. Do not leave both old and new protocols partially active for
longer than needed.

1. Add shared protocol constants and frame codec tests in `remote-exec-proto`.
2. Add broker-side frame decoder/encoder and unit tests.
3. Add target-info protocol version field and make broker require version `2`.
4. Convert Rust daemon export to framed output.
5. Convert broker remote export to require framed `COMPLETE`.
6. Refactor Rust import into stage-before-commit and add framed import.
7. Convert broker remote import to framed request bodies.
8. Add C++ frame codec and tests.
9. Refactor C++ export planning and stream framed export output.
10. Refactor C++ import into plan/complete/commit and add framed import.
11. Remove raw archive v1 transfer code paths and tests.
12. Run Rust, GNU make, BSD make, and Windows XP-compatible C++ gates.

If implementation pressure requires a smaller first slice, do export framing
first, with broker spooling framed exports to temp files before import. Direct
source-to-destination framed streaming can follow after both import
implementations prove cleanup correctness.

## Acceptance Criteria

The redesign is complete when:

- the broker no longer requests or accepts raw archive transfer responses;
- Rust daemon export cannot silently truncate a successful-looking archive on
  late failure;
- C++ daemon export streams without buffering the full archive in memory;
- Rust and C++ import do not commit output until framed `COMPLETE` is observed;
- framed `ERROR` and EOF-before-terminal leave no destination output;
- target-info reports transfer stream protocol version `2`;
- missing or wrong transfer stream version is rejected;
- public `transfer_files` behavior remains unchanged;
- C++ remains C++11, thread-based, no-`openat`, and XP-build compatible.

## Non-Goals

- no compatibility with old raw archive transfer bodies;
- no public MCP tool protocol change;
- no change to v4 port-forward protocol;
- no switch away from thread-based C++ daemon concurrency;
- no `openat` redesign.

## Notes

The key rule is that HTTP success is not transfer success. Transfer success is
only established by the terminal `COMPLETE` frame.

That rule should be visible in code structure. Export and import routes should
not treat socket EOF, HTTP chunk termination, or archive parser EOF as success
unless the framed transfer terminal state also says success.
