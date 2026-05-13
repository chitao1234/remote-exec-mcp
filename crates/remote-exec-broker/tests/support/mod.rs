#![allow(dead_code)]

pub mod certs;
pub mod fixture;
pub mod spawners;
pub mod streamable_http_child;
pub mod stub_daemon;
#[path = "../../../../tests/support/test_helpers.rs"]
pub mod test_helpers;
#[path = "../../../../tests/support/transfer_archive.rs"]
pub mod transfer_archive;

#[allow(
    unused_imports,
    reason = "Some broker integration test crates use this root re-export"
)]
pub use spawners::spawn_broker_with_plain_http_stub_daemon;

pub fn assert_correlated_tool_error(
    error: &str,
    tool: &str,
    target: Option<&str>,
    expected_suffix: &str,
) {
    assert!(
        error.starts_with("request_id=req_"),
        "missing request_id prefix in error: {error}"
    );
    assert!(
        error.contains(&format!(" tool={tool}")),
        "missing tool={tool} in error: {error}"
    );
    match target {
        Some(target) => assert!(
            error.contains(&format!(" target={target}: ")),
            "missing target={target} in error: {error}"
        ),
        None => assert!(
            !error.contains(" target="),
            "unexpected target context in error: {error}"
        ),
    }
    assert!(
        error.ends_with(expected_suffix),
        "error did not preserve expected suffix `{expected_suffix}`: {error}"
    );
}
