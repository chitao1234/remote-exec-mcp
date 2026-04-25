use std::time::{Duration, Instant};

use super::session::{LiveSession, OutputWait};

pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 10_000;
const BYTES_PER_TOKEN: usize = 4;
pub const EXIT_OUTPUT_IDLE_GRACE: Duration = Duration::from_millis(250);
pub const EXIT_OUTPUT_MAX_GRACE: Duration = Duration::from_secs(2);

pub struct OutputSnapshot {
    pub original_token_count: u32,
    pub output: String,
}

pub fn snapshot_output(raw: String, max_output_tokens: Option<u32>) -> OutputSnapshot {
    let original_token_count = approximate_token_count(raw.len());
    let output = truncate_to_token_limit(&raw, effective_max_output_tokens(max_output_tokens));
    OutputSnapshot {
        original_token_count,
        output,
    }
}

pub fn effective_max_output_tokens(max_output_tokens: Option<u32>) -> Option<u32> {
    Some(max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS))
}

fn approximate_token_count(bytes: usize) -> u32 {
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(BYTES_PER_TOKEN) as u32
    }
}

fn count_lines(raw: &str) -> usize {
    if raw.is_empty() {
        0
    } else {
        raw.split('\n').count() - usize::from(raw.ends_with('\n'))
    }
}

fn floor_char_boundary(raw: &str, max_bytes: usize) -> usize {
    let mut index = max_bytes.min(raw.len());
    while index > 0 && !raw.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(raw: &str, min_bytes: usize) -> usize {
    let mut index = min_bytes.min(raw.len());
    while index < raw.len() && !raw.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn suffix_start_for_budget(raw: &str, max_bytes: usize) -> usize {
    if max_bytes >= raw.len() {
        0
    } else {
        ceil_char_boundary(raw, raw.len() - max_bytes)
    }
}

fn truncation_prefix(raw: &str) -> String {
    format!("Total output lines: {}\n\n", count_lines(raw))
}

fn truncation_marker(truncated_tokens: u32) -> String {
    format!("\u{2026}{truncated_tokens} tokens truncated\u{2026}")
}

pub fn truncate_to_token_limit(raw: &str, max_output_tokens: Option<u32>) -> String {
    let Some(limit) = max_output_tokens else {
        return raw.to_string();
    };
    if limit == 0 {
        return String::new();
    }

    let max_output_bytes = limit as usize * BYTES_PER_TOKEN;
    if raw.len() <= max_output_bytes {
        return raw.to_string();
    }

    let prefix = truncation_prefix(raw);
    let mut truncated_tokens = approximate_token_count(raw.len());

    loop {
        let marker = truncation_marker(truncated_tokens);
        if max_output_bytes <= prefix.len() + marker.len() {
            return format!("{prefix}{marker}");
        }

        let payload_budget = max_output_bytes - prefix.len() - marker.len();
        let head_budget = payload_budget / 2;
        let tail_budget = payload_budget - head_budget;
        let head_end = floor_char_boundary(raw, head_budget);
        let tail_start = suffix_start_for_budget(raw, tail_budget).max(head_end);
        let next_truncated_tokens = approximate_token_count(tail_start.saturating_sub(head_end));

        if next_truncated_tokens == truncated_tokens {
            let mut rendered =
                String::with_capacity(prefix.len() + head_end + marker.len() + raw.len() - tail_start);
            rendered.push_str(&prefix);
            rendered.push_str(&raw[..head_end]);
            rendered.push_str(&marker);
            rendered.push_str(&raw[tail_start..]);
            return rendered;
        }

        truncated_tokens = next_truncated_tokens;
    }
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

#[cfg(test)]
mod tests {
    use super::{snapshot_output, truncate_to_token_limit};

    #[test]
    fn snapshot_output_uses_utf8_byte_count_for_original_token_count() {
        let snapshot = snapshot_output("\u{00e9}\u{00e9}\u{00e9}".to_string(), Some(100));

        assert_eq!(snapshot.original_token_count, 2);
        assert_eq!(snapshot.output, "\u{00e9}\u{00e9}\u{00e9}");
    }

    #[test]
    fn snapshot_output_adds_line_count_and_middle_marker_when_truncated() {
        let snapshot = snapshot_output("a".repeat(100), Some(15));

        assert_eq!(snapshot.original_token_count, 25);
        assert_eq!(
            snapshot.output,
            "Total output lines: 1\n\naaaaaa\u{2026}22 tokens truncated\u{2026}aaaaaa"
        );
    }

    #[test]
    fn snapshot_output_counts_lines_without_extra_trailing_blank_line() {
        let snapshot = snapshot_output("hello\nworld\n".to_string(), Some(1));

        assert!(snapshot.output.starts_with("Total output lines: 2\n\n"));
    }

    #[test]
    fn truncate_to_token_limit_returns_empty_string_for_zero_limit() {
        assert_eq!(truncate_to_token_limit("hello", Some(0)), "");
    }

    #[test]
    fn truncate_to_token_limit_preserves_trailing_newline_within_budget() {
        assert_eq!(truncate_to_token_limit("one two\n", Some(3)), "one two\n");
    }
}
