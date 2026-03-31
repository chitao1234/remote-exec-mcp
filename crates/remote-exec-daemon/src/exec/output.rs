use std::time::{Duration, Instant};

use super::session::LiveSession;

pub const EXIT_OUTPUT_GRACE: Duration = Duration::from_millis(100);

pub struct OutputSnapshot {
    pub original_token_count: u32,
    pub output: String,
}

pub fn snapshot_output(raw: String, max_output_tokens: Option<u32>) -> OutputSnapshot {
    let original_token_count = raw.split_whitespace().count() as u32;
    let output = truncate_to_token_limit(&raw, max_output_tokens);
    OutputSnapshot {
        original_token_count,
        output,
    }
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
    let mut end = raw.len();

    for (index, ch) in raw.char_indices() {
        if ch.is_whitespace() {
            in_token = false;
            continue;
        }

        if !in_token {
            seen += 1;
            if seen > limit {
                end = index;
                break;
            }
            in_token = true;
        }
    }

    raw[..end].trim_end().to_string()
}

pub async fn drain_after_exit(session: &mut LiveSession) -> anyhow::Result<String> {
    let deadline = Instant::now() + EXIT_OUTPUT_GRACE;
    let mut output = String::new();

    while Instant::now() < deadline {
        let chunk = session.read_available().await?;
        if !chunk.is_empty() {
            session.record_output(&chunk);
            output.push_str(&chunk);
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Ok(output)
}
