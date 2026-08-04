use std::future::Future;
use std::time::Duration;

pub const DEFAULT_TEST_TARGET: &str = "builder-a";
pub const XP_TEST_TARGET: &str = "builder-xp";
pub const TEST_BEARER_SECRET: &str = "shared-secret";

pub fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

pub fn utf16le_bom_bytes(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
    bytes
}

#[cfg(windows)]
pub fn windows_drive_prefix_and_rest(path: &std::path::Path) -> (char, String) {
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
pub fn msys_style_path(path: &std::path::Path) -> String {
    let (drive, rest) = windows_drive_prefix_and_rest(path);
    if rest.is_empty() {
        format!("/{}", drive.to_ascii_lowercase())
    } else {
        format!("/{}/{}", drive.to_ascii_lowercase(), rest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ReadinessWaitOutcome {
    Ready,
    Finished,
    TimedOut,
}

#[allow(dead_code)]
pub async fn poll_until_ready<Probe, ProbeFuture, Finished>(
    attempts: usize,
    interval: Duration,
    mut probe: Probe,
    finished: Finished,
) -> ReadinessWaitOutcome
where
    Probe: FnMut() -> ProbeFuture,
    ProbeFuture: Future<Output = bool>,
    Finished: Fn() -> bool,
{
    for _ in 0..attempts {
        if probe().await {
            return ReadinessWaitOutcome::Ready;
        }
        if finished() {
            return ReadinessWaitOutcome::Finished;
        }
        tokio::time::sleep(interval).await;
    }

    if finished() {
        ReadinessWaitOutcome::Finished
    } else {
        ReadinessWaitOutcome::TimedOut
    }
}
