mod certs;
pub mod fixture;
pub mod spawn;

#[cfg(windows)]
#[allow(
    dead_code,
    reason = "Shared across multiple Windows integration test crates"
)]
fn windows_drive_prefix_and_rest(path: &std::path::Path) -> (char, String) {
    let text = path.display().to_string().replace('\\', "/");
    let bytes = text.as_bytes();
    assert!(
        bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic(),
        "expected drive-qualified Windows path, got {text}"
    );

    let drive = bytes[0] as char;
    let rest = text[2..].trim_start_matches('/').to_string();
    (drive, rest)
}

#[cfg(windows)]
#[allow(
    dead_code,
    reason = "Shared across multiple Windows integration test crates"
)]
pub(crate) fn msys_style_path(path: &std::path::Path) -> String {
    let (drive, rest) = windows_drive_prefix_and_rest(path);
    if rest.is_empty() {
        format!("/{}", drive.to_ascii_lowercase())
    } else {
        format!("/{}/{}", drive.to_ascii_lowercase(), rest)
    }
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
