use std::time::{Duration, Instant};

use super::session::{LiveSession, OutputWait};

pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 10_000;
pub const EXIT_OUTPUT_IDLE_GRACE: Duration = Duration::from_millis(250);
pub const EXIT_OUTPUT_MAX_GRACE: Duration = Duration::from_secs(2);

pub struct OutputSnapshot {
    pub original_token_count: u32,
    pub output: String,
}

pub fn snapshot_output(raw: String, max_output_tokens: Option<u32>) -> OutputSnapshot {
    let original_token_count = raw.split_whitespace().count() as u32;
    let output = truncate_to_token_limit(&raw, effective_max_output_tokens(max_output_tokens));
    OutputSnapshot {
        original_token_count,
        output,
    }
}

pub fn effective_max_output_tokens(max_output_tokens: Option<u32>) -> Option<u32> {
    Some(max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS))
}

pub fn truncate_to_token_limit(raw: &str, max_output_tokens: Option<u32>) -> String {
    let Some(limit) = max_output_tokens else {
        return raw.to_string();
    };
    if limit == 0 {
        return String::new();
    }

    let mut seen = 0u32;
    let mut in_token = false;

    for (index, ch) in raw.char_indices() {
        if ch.is_whitespace() {
            in_token = false;
            continue;
        }

        if !in_token {
            seen += 1;
            if seen > limit {
                return raw[..index].trim_end().to_string();
            }
            in_token = true;
        }
    }

    raw.to_string()
}

pub async fn drain_after_exit(session: &mut LiveSession) -> anyhow::Result<String> {
    let mut output = String::new();
    let deadline = Instant::now() + EXIT_OUTPUT_MAX_GRACE;

    drain_available(session, &mut output).await?;

    while !session.output_closed() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }

        match session
            .wait_for_output(remaining.min(EXIT_OUTPUT_IDLE_GRACE))
            .await?
        {
            OutputWait::Chunk(chunk) => {
                session.record_output(&chunk);
                output.push_str(&chunk);
            }
            OutputWait::Closed | OutputWait::TimedOut => break,
        }
    }

    drain_available(session, &mut output).await?;
    Ok(output)
}

async fn drain_available(session: &mut LiveSession, output: &mut String) -> anyhow::Result<()> {
    let chunk = session.read_available().await?;
    if !chunk.is_empty() {
        session.record_output(&chunk);
        output.push_str(&chunk);
    }
    Ok(())
}
