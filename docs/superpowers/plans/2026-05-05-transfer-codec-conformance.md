# Transfer Codec Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Centralize transfer metadata wire semantics in `remote-exec-proto`, make Rust broker/daemon codecs thin adapters, and align the C++ daemon import metadata validation with the same contract.

**Architecture:** Keep public MCP transfer schemas and archive behavior unchanged. Add transport-neutral Rust transfer header helpers beside the existing RPC metadata structs, then adapt `reqwest` and Axum/http header maps around those helpers. Mirror the same required header, enum, boolean, and default rules in the C++ daemon route codec and prove them through route-level tests.

**Tech Stack:** Rust 2024, serde, reqwest, Axum/http, Tokio integration tests, C++11 daemon sources, C++17 host tests, cargo test, make

---

### Task 1: Add Proto-Owned Transfer Header Contract Tests

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify: `cargo test -p remote-exec-proto transfer_header`

**Testing approach:** `TDD`
Reason: the proto crate needs to own the canonical Rust transfer wire contract. These tests define the new helper API before broker or daemon adapters move to it.

- [ ] **Step 1: Add red tests to the existing `#[cfg(test)] mod tests` in `crates/remote-exec-proto/src/rpc.rs`**

```rust
use std::collections::BTreeMap;

use super::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferCompression, TransferExportMetadata,
    TransferHeaderErrorKind, TransferImportMetadata, TransferOverwriteMode, TransferSourceType,
    TransferSymlinkMode, parse_transfer_export_metadata, parse_transfer_import_metadata,
    transfer_export_header_pairs, transfer_import_header_pairs,
};

fn header_map(headers: &[(&'static str, &'static str)]) -> BTreeMap<&'static str, String> {
    headers
        .iter()
        .map(|(name, value)| (*name, (*value).to_string()))
        .collect()
}

fn lookup<'a>(
    headers: &'a BTreeMap<&'static str, String>,
) -> impl FnMut(&'static str) -> Result<Option<String>, super::TransferHeaderError> + 'a {
    move |name| Ok(headers.get(name).cloned())
}

#[test]
fn transfer_header_pairs_render_canonical_import_metadata() {
    let metadata = TransferImportMetadata {
        destination_path: "/tmp/output".to_string(),
        overwrite: TransferOverwriteMode::Replace,
        create_parent: true,
        source_type: TransferSourceType::Directory,
        compression: TransferCompression::Zstd,
        symlink_mode: TransferSymlinkMode::Follow,
    };

    assert_eq!(
        transfer_import_header_pairs(&metadata),
        vec![
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output".to_string()),
            (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
            (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
            (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
            (TRANSFER_COMPRESSION_HEADER, "zstd".to_string()),
            (TRANSFER_SYMLINK_MODE_HEADER, "follow".to_string()),
        ]
    );
}

#[test]
fn transfer_header_pairs_render_canonical_export_metadata() {
    let metadata = TransferExportMetadata {
        source_type: TransferSourceType::Multiple,
        compression: TransferCompression::None,
    };

    assert_eq!(
        transfer_export_header_pairs(&metadata),
        vec![
            (TRANSFER_SOURCE_TYPE_HEADER, "multiple".to_string()),
            (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
        ]
    );
}

#[test]
fn transfer_header_parser_reads_import_metadata_and_optional_defaults() {
    let headers = header_map(&[
        (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
        (TRANSFER_OVERWRITE_HEADER, "merge"),
        (TRANSFER_CREATE_PARENT_HEADER, "false"),
        (TRANSFER_SOURCE_TYPE_HEADER, "file"),
    ]);

    let parsed = parse_transfer_import_metadata(lookup(&headers)).unwrap();

    assert_eq!(
        parsed,
        TransferImportMetadata {
            destination_path: "/tmp/output".to_string(),
            overwrite: TransferOverwriteMode::Merge,
            create_parent: false,
            source_type: TransferSourceType::File,
            compression: TransferCompression::None,
            symlink_mode: TransferSymlinkMode::Preserve,
        }
    );
}

#[test]
fn transfer_header_parser_rejects_missing_required_import_headers() {
    for missing in [
        TRANSFER_DESTINATION_PATH_HEADER,
        TRANSFER_OVERWRITE_HEADER,
        TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_SOURCE_TYPE_HEADER,
    ] {
        let mut headers = header_map(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "true"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);
        headers.remove(missing);

        let err = parse_transfer_import_metadata(lookup(&headers)).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
        assert_eq!(err.header, missing);
    }
}

#[test]
fn transfer_header_parser_rejects_invalid_import_metadata_values() {
    for (header, value) in [
        (TRANSFER_OVERWRITE_HEADER, "clobber"),
        (TRANSFER_CREATE_PARENT_HEADER, "yes"),
        (TRANSFER_SOURCE_TYPE_HEADER, "folder"),
        (TRANSFER_COMPRESSION_HEADER, "gzip"),
        (TRANSFER_SYMLINK_MODE_HEADER, "copy"),
    ] {
        let mut headers = header_map(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "true"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
            (TRANSFER_COMPRESSION_HEADER, "none"),
            (TRANSFER_SYMLINK_MODE_HEADER, "preserve"),
        ]);
        headers.insert(header, value.to_string());

        let err = parse_transfer_import_metadata(lookup(&headers)).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, header);
    }
}

#[test]
fn transfer_header_parser_reads_export_metadata_defaults() {
    let headers = header_map(&[(TRANSFER_SOURCE_TYPE_HEADER, "directory")]);

    let parsed = parse_transfer_export_metadata(lookup(&headers)).unwrap();

    assert_eq!(
        parsed,
        TransferExportMetadata {
            source_type: TransferSourceType::Directory,
            compression: TransferCompression::None,
        }
    );
}

#[test]
fn transfer_header_parser_rejects_invalid_export_metadata() {
    let missing = BTreeMap::new();
    let err = parse_transfer_export_metadata(lookup(&missing)).unwrap_err();
    assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
    assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);

    let invalid = header_map(&[(TRANSFER_SOURCE_TYPE_HEADER, "folder")]);
    let err = parse_transfer_export_metadata(lookup(&invalid)).unwrap_err();
    assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
    assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
}
```

- [ ] **Step 2: Run the proto focused test and confirm red**

Run: `cargo test -p remote-exec-proto transfer_header`
Expected: FAIL to compile because the `TransferHeaderError`, header pair, parser, and enum wire helper APIs do not exist yet.

### Task 2: Implement the Proto Transfer Header Helpers

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify: `cargo test -p remote-exec-proto transfer_header`

**Testing approach:** `TDD`
Reason: this is the green implementation for the red proto contract tests.

- [ ] **Step 1: Add transport-neutral error, enum wire values, header pair rendering, and parsing helpers in `crates/remote-exec-proto/src/rpc.rs` near the transfer metadata types**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferHeaderErrorKind {
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferHeaderError {
    pub header: &'static str,
    pub kind: TransferHeaderErrorKind,
    pub message: String,
}

impl TransferHeaderError {
    pub fn missing(header: &'static str) -> Self {
        Self {
            header,
            kind: TransferHeaderErrorKind::Missing,
            message: format!("missing header `{header}`"),
        }
    }

    pub fn invalid(header: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            header,
            kind: TransferHeaderErrorKind::Invalid,
            message: format!("invalid header `{header}`: {message}"),
        }
    }
}

impl std::fmt::Display for TransferHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransferHeaderError {}
```

Add `wire_value()` and `from_wire_value()` impls for `TransferCompression`, `TransferSourceType`, `TransferOverwriteMode`, and `TransferSymlinkMode`, using only the accepted values from the approved spec.

Add these helper functions:

```rust
pub type TransferHeaderPairs = Vec<(&'static str, String)>;

pub fn transfer_export_header_pairs(metadata: &TransferExportMetadata) -> TransferHeaderPairs {
    vec![
        (
            TRANSFER_SOURCE_TYPE_HEADER,
            metadata.source_type.wire_value().to_string(),
        ),
        (
            TRANSFER_COMPRESSION_HEADER,
            metadata.compression.wire_value().to_string(),
        ),
    ]
}

pub fn transfer_import_header_pairs(metadata: &TransferImportMetadata) -> TransferHeaderPairs {
    vec![
        (
            TRANSFER_DESTINATION_PATH_HEADER,
            metadata.destination_path.clone(),
        ),
        (
            TRANSFER_OVERWRITE_HEADER,
            metadata.overwrite.wire_value().to_string(),
        ),
        (
            TRANSFER_CREATE_PARENT_HEADER,
            metadata.create_parent.to_string(),
        ),
        (
            TRANSFER_SOURCE_TYPE_HEADER,
            metadata.source_type.wire_value().to_string(),
        ),
        (
            TRANSFER_COMPRESSION_HEADER,
            metadata.compression.wire_value().to_string(),
        ),
        (
            TRANSFER_SYMLINK_MODE_HEADER,
            metadata.symlink_mode.wire_value().to_string(),
        ),
    ]
}

pub fn parse_transfer_export_metadata(
    mut header: impl FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
) -> Result<TransferExportMetadata, TransferHeaderError> {
    Ok(TransferExportMetadata {
        source_type: TransferSourceType::from_wire_value(
            required_transfer_header(&mut header, TRANSFER_SOURCE_TYPE_HEADER)?.as_str(),
        )
        .ok_or_else(|| {
            invalid_enum_header(
                TRANSFER_SOURCE_TYPE_HEADER,
                "expected one of `file`, `directory`, `multiple`",
            )
        })?,
        compression: optional_transfer_header(&mut header, TRANSFER_COMPRESSION_HEADER)?
            .as_deref()
            .map(|raw| {
                TransferCompression::from_wire_value(raw).ok_or_else(|| {
                    invalid_enum_header(
                        TRANSFER_COMPRESSION_HEADER,
                        "expected one of `none`, `zstd`",
                    )
                })
            })
            .transpose()?
            .unwrap_or_default(),
    })
}

pub fn parse_transfer_import_metadata(
    mut header: impl FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
) -> Result<TransferImportMetadata, TransferHeaderError> {
    Ok(TransferImportMetadata {
        destination_path: required_transfer_header(&mut header, TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_required_overwrite(&mut header)?,
        create_parent: parse_required_create_parent(&mut header)?,
        source_type: parse_required_source_type(&mut header)?,
        compression: parse_optional_compression(&mut header)?,
        symlink_mode: parse_optional_symlink_mode(&mut header)?,
    })
}
```

Use small private helpers for `required_transfer_header`, `optional_transfer_header`, `invalid_enum_header`, `parse_required_*`, and `parse_optional_*` to keep the public API compact.

- [ ] **Step 2: Run proto focused tests**

Run: `cargo test -p remote-exec-proto transfer_header`
Expected: PASS.

### Task 3: Add Rust Broker and Rust Daemon Adapter Conformance Tests

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-broker transfer_codec`, `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_invalid_create_parent_header_as_bad_request`

**Testing approach:** `TDD` for adapter behavior where the test is red against the new proto API, plus integration characterization for daemon cases that already match Rust behavior.
Reason: the broker adapter should use canonical proto values, and the daemon route must keep rejecting malformed import metadata while accepting optional defaults.

- [ ] **Step 1: Add broker unit tests in `crates/remote-exec-broker/src/tools/transfer/codec.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use remote_exec_proto::rpc::{
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER,
        TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER,
        TransferCompression, TransferImportMetadata, TransferOverwriteMode, TransferSourceType,
        TransferSymlinkMode,
    };

    #[test]
    fn transfer_codec_parses_export_metadata_from_reqwest_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "directory".parse().unwrap());

        let parsed = parse_export_metadata(&headers).unwrap();

        assert_eq!(parsed.source_type, TransferSourceType::Directory);
        assert_eq!(parsed.compression, TransferCompression::None);
    }

    #[test]
    fn transfer_codec_rejects_invalid_export_source_type_as_decode_error() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "folder".parse().unwrap());

        let err = parse_export_metadata(&headers).unwrap_err();

        assert!(matches!(err, DaemonClientError::Decode(_)));
        assert!(err.to_string().contains(TRANSFER_SOURCE_TYPE_HEADER));
    }

    #[tokio::test]
    async fn transfer_codec_applies_canonical_import_headers() {
        let client = reqwest::Client::new();
        let request = apply_import_headers(
            client.post("http://127.0.0.1/v1/transfer/import"),
            &TransferImportMetadata {
                destination_path: "/tmp/out".to_string(),
                overwrite: TransferOverwriteMode::Replace,
                create_parent: false,
                source_type: TransferSourceType::Multiple,
                compression: TransferCompression::Zstd,
                symlink_mode: TransferSymlinkMode::Skip,
            },
        )
        .body(reqwest::Body::from(Vec::new()))
        .build()
        .unwrap();

        assert_eq!(request.headers()[TRANSFER_DESTINATION_PATH_HEADER], "/tmp/out");
        assert_eq!(request.headers()[TRANSFER_OVERWRITE_HEADER], "replace");
        assert_eq!(request.headers()[TRANSFER_CREATE_PARENT_HEADER], "false");
        assert_eq!(request.headers()[TRANSFER_SOURCE_TYPE_HEADER], "multiple");
        assert_eq!(request.headers()[TRANSFER_COMPRESSION_HEADER], "zstd");
        assert_eq!(request.headers()[TRANSFER_SYMLINK_MODE_HEADER], "skip");
    }
}
```

- [ ] **Step 2: Add Rust daemon import metadata route conformance tests in `crates/remote-exec-daemon/tests/transfer_rpc.rs` after `import_rejects_missing_destination_header_as_bad_request`**

```rust
#[tokio::test]
async fn import_rejects_missing_create_parent_header_as_bad_request() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("dest.txt");
    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            raw_tar_file_with_path(Path::new(".remote-exec-file"), b"artifact\n"),
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "bad_request");
    assert!(err.message.contains(TRANSFER_CREATE_PARENT_HEADER));
    assert!(!destination.exists());
}

#[tokio::test]
async fn import_rejects_invalid_create_parent_header_as_bad_request() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("dest.txt");
    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "yes".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            raw_tar_file_with_path(Path::new(".remote-exec-file"), b"artifact\n"),
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "bad_request");
    assert!(err.message.contains(TRANSFER_CREATE_PARENT_HEADER));
    assert!(!destination.exists());
}

#[tokio::test]
async fn import_rejects_invalid_metadata_enum_header_as_bad_request() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("dest.txt");
    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "clobber".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            raw_tar_file_with_path(Path::new(".remote-exec-file"), b"artifact\n"),
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "bad_request");
    assert!(err.message.contains(TRANSFER_OVERWRITE_HEADER));
    assert!(!destination.exists());
}
```

- [ ] **Step 3: Run focused broker and daemon tests**

Run: `cargo test -p remote-exec-broker transfer_codec`
Expected: FAIL until the broker codec uses the new proto helpers.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_invalid_create_parent_header_as_bad_request`
Expected: PASS before refactor and after refactor, proving the existing Rust daemon behavior stays stable.

### Task 4: Convert Rust Broker and Daemon Codecs to Thin Adapters

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/codec.rs`
- Test/Verify: `cargo test -p remote-exec-broker transfer_codec`, `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_create_parent_header_as_bad_request import_rejects_invalid_create_parent_header_as_bad_request import_rejects_invalid_metadata_enum_header_as_bad_request`

**Testing approach:** `TDD`
Reason: this is the adapter implementation that turns the broker red tests green while preserving Rust daemon route behavior.

- [ ] **Step 1: Update the broker codec adapter to delegate wire semantics to proto**

```rust
use remote_exec_proto::rpc::{
    TransferCompression, TransferExportMetadata, TransferHeaderError, TransferImportMetadata,
    parse_transfer_export_metadata, transfer_import_header_pairs,
};

pub(crate) fn parse_export_metadata(
    headers: &reqwest::header::HeaderMap,
) -> Result<TransferExportMetadata, DaemonClientError> {
    parse_transfer_export_metadata(|name| reqwest_header_string(headers, name))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

pub(crate) fn apply_import_headers(
    builder: reqwest::RequestBuilder,
    metadata: &TransferImportMetadata,
) -> reqwest::RequestBuilder {
    transfer_import_header_pairs(metadata)
        .into_iter()
        .fold(builder, |builder, (name, value)| builder.header(name, value))
}

pub(crate) fn compression_header_value(compression: &TransferCompression) -> &'static str {
    compression.wire_value()
}

fn reqwest_header_string(
    headers: &reqwest::header::HeaderMap,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|err| TransferHeaderError::invalid(name, err.to_string()))
        })
        .transpose()
}
```

Remove the broker-local enum match helpers and JSON-string enum parsing helpers after the proto helpers replace them.

- [ ] **Step 2: Update the Rust daemon codec adapter to delegate wire semantics to proto**

```rust
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferCompression, TransferExportMetadata, TransferHeaderError,
    TransferImportMetadata, TransferSourceType, parse_transfer_import_metadata,
    transfer_export_header_pairs,
};

pub(crate) fn apply_export_headers(
    builder: axum::http::response::Builder,
    metadata: &TransferExportMetadata,
) -> axum::http::response::Builder {
    transfer_export_header_pairs(metadata)
        .into_iter()
        .fold(builder, |builder, (name, value)| builder.header(name, value))
}

pub(crate) fn parse_import_metadata(
    headers: &HeaderMap,
) -> Result<TransferImportMetadata, (StatusCode, Json<RpcErrorBody>)> {
    parse_transfer_import_metadata(|name| axum_header_string(headers, name))
        .map_err(|err| bad_request(err.to_string()))
}

pub(crate) fn source_type_header_value(source_type: &TransferSourceType) -> &'static str {
    source_type.wire_value()
}

pub(crate) fn compression_header_value(compression: &TransferCompression) -> &'static str {
    compression.wire_value()
}

fn axum_header_string(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|err| TransferHeaderError::invalid(name, err.to_string()))
        })
        .transpose()
}
```

Remove the daemon-local required/optional enum and boolean parsing helpers after delegation.

- [ ] **Step 3: Run focused Rust adapter and daemon tests**

Run: `cargo test -p remote-exec-broker transfer_codec`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_create_parent_header_as_bad_request import_rejects_invalid_create_parent_header_as_bad_request import_rejects_invalid_metadata_enum_header_as_bad_request`
Expected: PASS.

### Task 5: Add C++ Route-Level Import Metadata Conformance Tests

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: this is the concrete cross-implementation drift. Missing or invalid C++ import metadata must fail before archive import, and omitted optional metadata must still use defaults.

- [ ] **Step 1: Add helper functions near the existing C++ route test helpers**

```cpp
static HttpRequest transfer_import_request(
    const fs::path& destination,
    const std::string& archive
) {
    HttpRequest request;
    request.method = "POST";
    request.path = "/v1/transfer/import";
    request.headers["x-remote-exec-source-type"] = "file";
    request.headers["x-remote-exec-destination-path"] = destination.string();
    request.headers["x-remote-exec-overwrite"] = "replace";
    request.headers["x-remote-exec-create-parent"] = "true";
    request.headers["x-remote-exec-symlink-mode"] = "preserve";
    request.headers["x-remote-exec-compression"] = "none";
    request.body = archive;
    return request;
}

static void assert_bad_request_for_transfer_import(
    AppState& state,
    const HttpRequest& request,
    const fs::path& destination,
    const std::string& message_fragment
) {
    const HttpResponse response = route_request(state, request);
    assert(response.status == 400);
    const Json body = Json::parse(response.body);
    assert(body.at("code").get<std::string>() == "bad_request");
    assert(
        body.at("message").get<std::string>().find(message_fragment) != std::string::npos
    );
    assert(!fs::exists(destination));
}
```

- [ ] **Step 2: Add C++ conformance assertions after the first successful import route test**

```cpp
    HttpRequest optional_defaults_import =
        transfer_import_request(root / "transfer-defaults.txt", export_response.body);
    optional_defaults_import.headers.erase("x-remote-exec-symlink-mode");
    optional_defaults_import.headers.erase("x-remote-exec-compression");
    const HttpResponse optional_defaults_response =
        route_request(state, optional_defaults_import);
    assert(optional_defaults_response.status == 200);
    assert(read_text_file(root / "transfer-defaults.txt") == "route transfer payload");

    HttpRequest missing_create_parent =
        transfer_import_request(root / "missing-create-parent.txt", export_response.body);
    missing_create_parent.headers.erase("x-remote-exec-create-parent");
    assert_bad_request_for_transfer_import(
        state,
        missing_create_parent,
        root / "missing-create-parent.txt",
        "x-remote-exec-create-parent"
    );

    HttpRequest invalid_create_parent =
        transfer_import_request(root / "invalid-create-parent.txt", export_response.body);
    invalid_create_parent.headers["x-remote-exec-create-parent"] = "yes";
    assert_bad_request_for_transfer_import(
        state,
        invalid_create_parent,
        root / "invalid-create-parent.txt",
        "x-remote-exec-create-parent"
    );

    HttpRequest invalid_source_type =
        transfer_import_request(root / "invalid-source-type.txt", export_response.body);
    invalid_source_type.headers["x-remote-exec-source-type"] = "folder";
    assert_bad_request_for_transfer_import(
        state,
        invalid_source_type,
        root / "invalid-source-type.txt",
        "x-remote-exec-source-type"
    );

    HttpRequest invalid_overwrite =
        transfer_import_request(root / "invalid-overwrite.txt", export_response.body);
    invalid_overwrite.headers["x-remote-exec-overwrite"] = "clobber";
    assert_bad_request_for_transfer_import(
        state,
        invalid_overwrite,
        root / "invalid-overwrite.txt",
        "x-remote-exec-overwrite"
    );

    HttpRequest invalid_compression =
        transfer_import_request(root / "invalid-compression.txt", export_response.body);
    invalid_compression.headers["x-remote-exec-compression"] = "gzip";
    assert_bad_request_for_transfer_import(
        state,
        invalid_compression,
        root / "invalid-compression.txt",
        "x-remote-exec-compression"
    );

    HttpRequest invalid_symlink_mode =
        transfer_import_request(root / "invalid-symlink-mode.txt", export_response.body);
    invalid_symlink_mode.headers["x-remote-exec-symlink-mode"] = "copy";
    assert_bad_request_for_transfer_import(
        state,
        invalid_symlink_mode,
        root / "invalid-symlink-mode.txt",
        "x-remote-exec-symlink-mode"
    );
```

- [ ] **Step 3: Run the C++ route test and confirm red**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: FAIL because the current C++ daemon treats missing `x-remote-exec-create-parent` as `false` and reports invalid metadata through later transfer operation errors instead of `bad_request`.

### Task 6: Implement C++ Transfer Metadata Validation

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/rpc_failures.h`
- Modify: `crates/remote-exec-daemon-cpp/src/rpc_failures.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`, `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** `TDD`
Reason: this is the green implementation for the route-level C++ conformance failures.

- [ ] **Step 1: Add a C++ transfer `BadRequest` RPC code**

```cpp
enum class TransferRpcCode {
    BadRequest,
    SandboxDenied,
    PathNotAbsolute,
    DestinationExists,
    ParentMissing,
    DestinationUnsupported,
    CompressionUnsupported,
    SourceUnsupported,
    SourceMissing,
    Internal,
    TransferFailed,
};
```

Map it in `transfer_error_code_name`:

```cpp
case TransferRpcCode::BadRequest:
    return "bad_request";
```

`transfer_error_status` should keep returning `400` through the existing default branch.

- [ ] **Step 2: Replace ad hoc C++ metadata parsing with explicit validation in `transfer_http_codec.cpp`**

```cpp
namespace {

const char* DESTINATION_PATH_HEADER = "x-remote-exec-destination-path";
const char* OVERWRITE_HEADER = "x-remote-exec-overwrite";
const char* CREATE_PARENT_HEADER = "x-remote-exec-create-parent";
const char* SOURCE_TYPE_HEADER = "x-remote-exec-source-type";
const char* COMPRESSION_HEADER = "x-remote-exec-compression";
const char* SYMLINK_MODE_HEADER = "x-remote-exec-symlink-mode";

std::string missing_header_message(const char* name) {
    return std::string("missing header `") + name + "`";
}

std::string invalid_header_message(const char* name, const std::string& detail) {
    return std::string("invalid header `") + name + "`: " + detail;
}

std::string required_header(const HttpRequest& request, const char* name) {
    const std::map<std::string, std::string>::const_iterator it = request.headers.find(name);
    if (it == request.headers.end()) {
        throw TransferFailure(
            TransferRpcCode::BadRequest,
            missing_header_message(name)
        );
    }
    return it->second;
}

std::string optional_header_or(
    const HttpRequest& request,
    const char* name,
    const std::string& fallback
) {
    const std::map<std::string, std::string>::const_iterator it = request.headers.find(name);
    if (it == request.headers.end()) {
        return fallback;
    }
    return it->second;
}

void require_one_of(
    const char* name,
    const std::string& value,
    const char* first,
    const char* second,
    const char* third
) {
    if (value == first || value == second || value == third) {
        return;
    }
    throw TransferFailure(
        TransferRpcCode::BadRequest,
        invalid_header_message(name, "unsupported value `" + value + "`")
    );
}

void require_one_of(
    const char* name,
    const std::string& value,
    const char* first,
    const char* second
) {
    if (value == first || value == second) {
        return;
    }
    throw TransferFailure(
        TransferRpcCode::BadRequest,
        invalid_header_message(name, "unsupported value `" + value + "`")
    );
}

bool parse_create_parent(const std::string& value) {
    if (value == "true") {
        return true;
    }
    if (value == "false") {
        return false;
    }
    throw TransferFailure(
        TransferRpcCode::BadRequest,
        invalid_header_message(CREATE_PARENT_HEADER, "expected `true` or `false`")
    );
}

}  // namespace
```

Then update `parse_transfer_import_metadata`:

```cpp
TransferImportMetadata parse_transfer_import_metadata(const HttpRequest& request) {
    TransferImportMetadata metadata;
    metadata.destination_path = required_header(request, DESTINATION_PATH_HEADER);
    metadata.overwrite = required_header(request, OVERWRITE_HEADER);
    require_one_of(OVERWRITE_HEADER, metadata.overwrite, "fail", "merge", "replace");
    metadata.create_parent = parse_create_parent(required_header(request, CREATE_PARENT_HEADER));
    metadata.source_type = required_header(request, SOURCE_TYPE_HEADER);
    require_one_of(SOURCE_TYPE_HEADER, metadata.source_type, "file", "directory", "multiple");
    metadata.compression = optional_header_or(request, COMPRESSION_HEADER, "none");
    require_one_of(COMPRESSION_HEADER, metadata.compression, "none", "zstd");
    metadata.symlink_mode = optional_header_or(request, SYMLINK_MODE_HEADER, "preserve");
    require_one_of(SYMLINK_MODE_HEADER, metadata.symlink_mode, "preserve", "follow", "skip");
    return metadata;
}
```

- [ ] **Step 3: Run focused C++ route and streaming tests**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS, proving the streaming route still parses valid import metadata.

### Task 7: Run Focused Conformance Verification

**Files:**
- Modify: none unless verification reveals a defect
- Test/Verify: `cargo test -p remote-exec-proto`, `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-daemon --test transfer_rpc`, `make -C crates/remote-exec-daemon-cpp test-host-transfer`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** `existing tests + targeted verification`
Reason: the change touches a shared internal RPC contract, live Rust daemon route behavior, broker transfer routing, and C++ daemon route behavior.

- [ ] **Step 1: Run focused Rust transfer checks**

Run: `cargo test -p remote-exec-proto`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS.

- [ ] **Step 2: Run focused C++ transfer and route checks**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 3: Run formatting and broader Rust checks if focused tests pass**

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

