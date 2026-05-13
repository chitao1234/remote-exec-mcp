use std::future::Future;
use std::time::Duration;

pub fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessWaitOutcome {
    Ready,
    Finished,
    TimedOut,
}

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
