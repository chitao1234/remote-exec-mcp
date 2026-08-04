#[cfg(feature = "tls")]
mod certs;
pub mod fixture;
pub mod spawn;

use std::path::Path;

#[allow(
    unused_imports,
    reason = "Different daemon integration tests use different shared helper subsets"
)]
use encoding_rs::Encoding;

#[allow(
    unused_imports,
    reason = "Different daemon integration tests use different shared helper subsets"
)]
pub mod test_helpers {
    pub use remote_exec_test_support::test_helpers::*;
}

#[allow(
    unused_imports,
    reason = "Different daemon integration tests use different shared helper subsets"
)]
pub mod transfer_archive {
    pub use remote_exec_test_support::transfer_archive::*;
}

#[cfg(windows)]
#[allow(
    unused_imports,
    reason = "Different daemon integration tests use different shared helper subsets"
)]
pub(crate) use remote_exec_test_support::test_helpers::windows_drive_prefix_and_rest;

#[cfg(windows)]
#[allow(
    unused_imports,
    reason = "Different daemon integration tests use different shared helper subsets"
)]
pub(crate) use remote_exec_test_support::test_helpers::msys_style_path;

#[allow(
    dead_code,
    reason = "Shared across multiple daemon integration test crates"
)]
pub const ENCODING_AUTODETECT_CONFIG: &str =
    "experimental_apply_patch_target_encoding_autodetect = true";

#[allow(
    dead_code,
    reason = "Shared across multiple daemon integration test crates"
)]
pub fn encoded_bytes(encoding: &'static Encoding, text: &str) -> Vec<u8> {
    let (encoded, _, had_errors) = encoding.encode(text);
    assert!(
        !had_errors,
        "test text should encode as {}",
        encoding.name()
    );
    encoded.into_owned()
}

/// Renders a `[sandbox.<section>]` config fragment allowing `allow` (and
/// optionally denying `deny`).
#[allow(
    dead_code,
    reason = "Shared across multiple daemon integration test crates"
)]
pub fn sandbox_allow_config(section: &str, allow: &Path, deny: Option<&Path>) -> String {
    let allow = toml::Value::Array(vec![toml::Value::String(allow.display().to_string())]);
    let mut config = format!("[sandbox.{section}]\nallow = {allow}\n");
    if let Some(deny) = deny {
        let deny = toml::Value::Array(vec![toml::Value::String(deny.display().to_string())]);
        config.push_str(&format!("deny = {deny}\n"));
    }
    config
}

#[cfg(windows)]
#[allow(
    dead_code,
    reason = "Shared across multiple Windows integration test crates"
)]
pub(crate) fn cygwin_style_path(path: &std::path::Path) -> String {
    let (drive, rest) = windows_drive_prefix_and_rest(path);
    if rest.is_empty() {
        format!("/cygdrive/{}", drive.to_ascii_lowercase())
    } else {
        format!("/cygdrive/{}/{}", drive.to_ascii_lowercase(), rest)
    }
}

#[cfg(windows)]
#[allow(
    dead_code,
    reason = "Shared across multiple Windows integration test crates"
)]
pub(crate) fn posix_root_relative_path(root: &std::path::Path, path: &std::path::Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or_else(|_| {
        panic!(
            "expected `{}` to be within synthetic posix root `{}`",
            path.display(),
            root.display()
        )
    });
    let text = relative.display().to_string().replace('\\', "/");
    if text.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", text.trim_start_matches('/'))
    }
}
