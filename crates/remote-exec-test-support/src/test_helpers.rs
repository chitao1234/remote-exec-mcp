use std::future::Future;
use std::time::Duration;

pub fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

pub fn utf16le_bom_bytes(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
    bytes
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
