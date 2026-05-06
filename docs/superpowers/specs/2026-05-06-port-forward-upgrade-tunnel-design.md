# Port Forward Upgrade Tunnel Design

## Goal

Rework the internal broker-daemon port forwarding protocol so `forward_ports` keeps the existing public MCP schema while using a lower-overhead, less chatty, full-duplex transport for both Rust and C++ daemons.

The new protocol replaces the current per-operation HTTP JSON RPC flow with an HTTP/1.1 Upgrade tunnel and removes the daemon-side lease renewal model entirely.

## Current Problems

The current broker-daemon port forwarding implementation is correct but expensive:

- TCP forwarding uses repeated `connection/read` and `connection/write` HTTP requests for every 64 KiB chunk.
- TCP and UDP payloads are base64 encoded inside JSON, adding CPU and bandwidth overhead on the hot path.
- UDP forwarding is serialized request/reply behavior, not a full-duplex datagram relay.
- The broker renews daemon-side forward leases every second for listeners and active TCP connections.
- Broker crashes are handled by lease expiry instead of transport ownership, so cleanup is intentionally delayed and renewal traffic remains constant while forwards are open.

## Non-Goals

- No public MCP schema changes for `forward_ports`.
- No direct daemon-to-daemon connectivity.
- No fallback to the older per-operation daemon protocol.
- No compatibility path for older daemons that lack the tunnel protocol.
- No HTTP/2, WebSocket, or public daemon API redesign.

## Chosen Approach

Use a private HTTP/1.1 Upgrade endpoint:

```text
POST /v1/port/tunnel
Connection: Upgrade
Upgrade: remote-exec-port-tunnel
X-Remote-Exec-Port-Tunnel-Version: 1
```

After a `101 Switching Protocols` response, the broker and daemon exchange binary frames over the upgraded socket. The upgraded connection is full duplex: either side may send frames at any time, and the broker relays data frames between the listen side and connect side without issuing HTTP requests per chunk or per datagram.

Both Rust and C++ daemons implement this tunnel in the same change. The broker requires tunnel support and does not fall back to the older `/v1/port/*` operation endpoints.

## Capability Contract

`TargetInfoResponse` gains an internal capability field:

```rust
#[serde(default)]
pub port_forward_protocol_version: u32,
```

Rust and C++ daemons return `port_forward_protocol_version = 2` with `supports_port_forward = true`.
Version `2` means "tunnel-based forwarding" at the daemon RPC capability layer; the first binary frame format carried by that tunnel is versioned independently as `REPFWD1`.

The broker treats `supports_port_forward = true` plus `port_forward_protocol_version >= 2` as required for remote port forwarding. Missing or lower versions fail before opening a forward.

This is an internal broker-daemon RPC contract change, not a public MCP schema change.

## Old Protocol Removal

The following old daemon port-forward HTTP routes are removed from broker use and should be removed from the Rust and C++ daemon routers as part of the rework:

- `/v1/port/listen`
- `/v1/port/listen/accept`
- `/v1/port/listen/close`
- `/v1/port/lease/renew`
- `/v1/port/connect`
- `/v1/port/connection/read`
- `/v1/port/connection/write`
- `/v1/port/connection/close`
- `/v1/port/udp/read`
- `/v1/port/udp/write`

The `PortForwardLease` model does not survive the new protocol. Daemon resources are scoped to the upgraded tunnel lifetime instead.

## Tunnel Resource Ownership

Every listener, connector socket, accepted TCP connection, and outbound TCP connection created through a tunnel is owned by that tunnel.

Cleanup rules:

- Broker closes a forward: broker sends close frames for owned listener/connector resources and closes its tunnel handles.
- Broker process exits or crashes: upgraded socket closes; daemon immediately closes all resources owned by the tunnel.
- Daemon restarts or tunnel breaks: broker marks the public forward failed and closes the paired side.
- TCP peer closes: daemon sends EOF/CLOSE for that stream; broker propagates closure to the paired stream.
- No periodic renewals are sent.

This makes cleanup transport-driven and removes delayed lease expiry from normal operation.

## Frame Format

The tunnel starts with an 8-byte preface from the broker:

```text
REPFWD1\n
```

Then both sides exchange frames with a fixed 16-byte big-endian header:

```text
u8  frame_type
u8  flags
u16 reserved
u32 stream_id
u32 meta_len
u32 data_len
```

The header is followed by:

1. `meta_len` bytes of UTF-8 JSON metadata.
2. `data_len` bytes of raw binary payload.

Only control metadata is JSON. TCP bytes and UDP datagram payloads are never base64 encoded.

Limits:

- `meta_len` is capped at a small fixed limit such as 16 KiB.
- `data_len` is capped at a transport chunk limit such as 64 KiB or 256 KiB.
- Unknown frame types, oversized frames, malformed JSON metadata, and invalid state transitions close the affected resource and return an `ERROR` frame when possible.

## Stream IDs

Stream IDs are scoped to one tunnel.

- Broker-initiated resources use odd stream IDs.
- Daemon-initiated resources use even stream IDs.
- `stream_id = 0` is reserved for tunnel-level errors/control and is not used for data streams.

The side that initiates a resource chooses the stream ID:

- Broker chooses stream IDs for TCP listeners, outbound TCP connects, UDP listeners, and UDP connector sockets.
- Daemon chooses stream IDs for TCP connections accepted by a daemon-owned TCP listener.

## Frame Types

The exact numeric values are implementation details, but the protocol has these semantic frame types:

### Tunnel

- `ERROR`: reports an error. Metadata includes `code`, `message`, and optionally `fatal`.
- `CLOSE`: closes the resource identified by `stream_id`.

### TCP

- `TCP_LISTEN`: broker asks daemon to bind a TCP listener. Metadata includes `endpoint`.
- `TCP_LISTEN_OK`: daemon reports the bound endpoint. Metadata includes `endpoint`.
- `TCP_ACCEPT`: daemon reports an accepted TCP connection. `stream_id` is the accepted connection; metadata includes `listener_stream_id` and optionally `peer`.
- `TCP_CONNECT`: broker asks daemon to open an outbound TCP connection. Metadata includes `endpoint`.
- `TCP_CONNECT_OK`: daemon confirms outbound connection establishment.
- `TCP_DATA`: raw TCP payload in the data section.
- `TCP_EOF`: read half reached EOF.

### UDP

- `UDP_BIND`: broker asks daemon to bind a UDP socket. Metadata includes `endpoint`.
- `UDP_BIND_OK`: daemon reports the bound endpoint. Metadata includes `endpoint`.
- `UDP_DATAGRAM`: raw UDP datagram in the data section. Metadata includes `peer`.

There is no serialized UDP request/reply operation. UDP sockets are full duplex: datagrams may arrive from either side at any time.

## TCP Forwarding Flow

For a public TCP forward:

1. Broker opens a tunnel to the listen side if remote, or an in-process local tunnel adapter if `"local"`.
2. Broker opens a tunnel to the connect side if remote, or an in-process local tunnel adapter if `"local"`.
3. Broker sends `TCP_LISTEN` on the listen tunnel.
4. Daemon replies `TCP_LISTEN_OK`; broker returns the public `ForwardPortEntry`.
5. When a client connects to the listener, daemon sends `TCP_ACCEPT`.
6. Broker sends `TCP_CONNECT` to the connect side with the configured destination endpoint.
7. After `TCP_CONNECT_OK`, broker pairs the accepted stream with the outbound stream.
8. `TCP_DATA` frames are relayed in both directions concurrently.
9. `TCP_EOF`, `CLOSE`, tunnel failure, or daemon error closes the paired stream and releases both sides.

This removes per-chunk HTTP request/response cycles.

## UDP Forwarding Flow

For a public UDP forward:

1. Broker opens a full-duplex tunnel to each side.
2. Broker sends `UDP_BIND` to the listen side using the public listen endpoint.
3. Broker returns the public `ForwardPortEntry` after `UDP_BIND_OK`.
4. When the listen side receives a datagram from peer `P`, daemon sends `UDP_DATAGRAM` on the listen UDP stream with metadata `peer = P`.
5. Broker creates or reuses a per-peer UDP connector stream on the connect side by sending `UDP_BIND` with endpoint `127.0.0.1:0` or the platform-normalized equivalent.
6. Broker sends the datagram to the fixed public `connect_endpoint` on that connector stream as `UDP_DATAGRAM`.
7. Replies received by that connector stream are relayed back to listen peer `P`.
8. Per-peer connector streams are closed after an idle timeout and always close when the public forward closes or either tunnel drops.

This replaces the old blocking request/reply UDP behavior with full-duplex datagram relay semantics and supports concurrent UDP peers without response misassociation.

## Broker Architecture

Add a broker-side tunnel abstraction used by `forward_ports`:

```text
PortTunnel
  send(Frame)
  recv() -> Frame
  close_stream(stream_id)
```

Implementations:

- `RemotePortTunnel`: opens `/v1/port/tunnel` via `reqwest`, validates `101 Switching Protocols`, upgrades the response, and reads/writes binary frames.
- `LocalPortTunnel`: uses the same Rust host tunnel service in-process through a Tokio duplex stream, so `"local"` side behavior follows the same state machine without HTTP.

The existing `SideHandle` port operation methods are replaced with tunnel creation and capability validation. The public `PortForwardStore` continues to own public `forward_id` state and cancellation tokens.

For TCP, the broker maintains a map of paired stream IDs between the listen-side tunnel and connect-side tunnel. For UDP, the broker maintains a map from listen peer to connect-side UDP connector stream ID.

## Rust Daemon Architecture

Add a Rust host tunnel service, likely under `remote-exec-host::port_forward::tunnel`, that operates on an `AsyncRead + AsyncWrite` stream and `HostRuntimeState`.

Responsibilities:

- Decode and validate frames.
- Bind TCP/UDP listeners.
- Open outbound TCP connections.
- Spawn full-duplex socket/frame pumps.
- Emit accept, data, EOF, datagram, close, and error frames.
- Track all resources owned by the tunnel and close them on tunnel shutdown.

The Rust daemon HTTP layer adds `/v1/port/tunnel` and enables Hyper upgrades by changing `serve_connection(...)` to `serve_connection(...).with_upgrades()` for both plain HTTP and TLS transports.

The old lease store and per-operation port-forward HTTP handlers are removed from the final implementation. The daemon should not expose `/v1/port/lease/renew`, and the broker should not contain code paths that call any older per-operation `/v1/port/*` forwarding route.

## C++ Daemon Architecture

Add a C++11-compatible tunnel implementation that reuses the existing socket helpers and `PortForwardStore` concepts where useful but changes ownership from lease-based maps to tunnel-scoped resources.

Responsibilities:

- Parse HTTP/1.1 Upgrade requests for `/v1/port/tunnel`.
- Send `101 Switching Protocols`.
- Enter full-duplex frame handling on the accepted socket.
- Use threads for read/write pumps and accepted TCP connections, consistent with the existing C++ daemon style.
- Close all tunnel-owned sockets when the upgraded socket closes.

The C++ daemon no longer needs lease renewal for port-forward resources. Existing C++ tests for lease expiry should be replaced with tunnel-drop cleanup tests.

## Error Handling

Errors are scoped as narrowly as possible:

- A malformed tunnel preface or frame header is a fatal tunnel error.
- Invalid `TCP_LISTEN`, `TCP_CONNECT`, or `UDP_BIND` metadata returns an `ERROR` for that `stream_id`.
- Socket read/write errors produce `ERROR` or `CLOSE` and release the affected stream.
- Tunnel read/write failure releases every resource owned by that tunnel.
- Broker-visible public forward status becomes `Failed` when a background tunnel or paired stream fails outside an intentional close.

Existing error wording that public tests depend on should remain stable at the public `forward_ports` layer where practical.

## Security

The tunnel endpoint uses the same daemon authentication and transport protections as existing broker-daemon RPC:

- Rust daemon mTLS behavior remains unchanged.
- Plain HTTP bearer auth behavior remains unchanged.
- C++ daemon bearer auth behavior remains unchanged.
- HTTP/1.1-only validation applies to the tunnel endpoint.

The tunnel does not grant new permissions beyond existing `forward_ports`: selecting a target still grants network access from that side.

## Documentation Updates

Update:

- `README.md`: explain that `forward_ports` uses an internal HTTP/1.1 Upgrade tunnel, no public schema changes, and daemon resources are cleaned up on tunnel drop rather than lease expiry.
- `configs/broker.example.toml`: update comments that mention lease-based daemon reclamation.
- `crates/remote-exec-daemon-cpp/README.md`: document the C++ tunnel support and removal of lease renewal behavior.
- `skills/using-remote-exec-mcp/SKILL.md` if it documents `forward_ports` lifecycle semantics.

## Testing Strategy

Use TDD at clear seams:

1. Frame codec tests in Rust and C++:
   - Encodes/decodes headers, metadata, and binary payloads.
   - Rejects oversized metadata/data.
   - Preserves arbitrary binary bytes without base64.

2. Rust daemon tunnel tests:
   - Upgrade handshake succeeds for `/v1/port/tunnel`.
   - HTTP/1.0 or missing upgrade headers fail.
   - TCP listen/connect/data forwarding works over frames.
   - UDP full-duplex datagrams work with at least two peers.
   - Dropping the upgraded tunnel promptly releases listeners and sockets.

3. C++ daemon tunnel tests:
   - HTTP Upgrade response is correct.
   - TCP full-duplex data flow works over frames.
   - UDP full-duplex per-peer connector flow works.
   - Dropping the upgraded socket reclaims listeners without lease expiry.

4. Broker public tests:
   - Existing public `forward_ports` tests continue to pass unchanged from the MCP caller perspective.
   - Real Rust daemon and real C++ daemon e2e tests prove no public schema changes.
   - A capability test proves an older or missing tunnel protocol version is rejected rather than falling back.

5. Regression checks:
   - No broker code sends `/v1/port/lease/renew`.
   - No broker hot path uses per-chunk `/v1/port/connection/read` or `/v1/port/connection/write`.

## Open Implementation Notes

- Prefer keeping the frame codec small and dependency-light.
- Use Tokio split halves and bounded channels on Rust to avoid one blocked stream starving unrelated streams.
- Use a single writer task per tunnel so frames from multiple socket pump tasks are serialized safely.
- Apply backpressure through bounded channels rather than unbounded buffering.
- Pick conservative idle timeout defaults for UDP per-peer connector streams and make them constants, not public configuration.
- Keep old public `forward_id` behavior and list/close semantics unchanged.
