use super::matcher;
use super::parser::UpdateChunk;

pub fn apply_hunks(
    current: &str,
    hunks: &[UpdateChunk],
    line_ending: &str,
) -> anyhow::Result<String> {
    let original_lines = split_current_lines(current);
    let replacements = plan_replacements(&original_lines, hunks)?;
    let mut lines = original_lines;

    for replacement in replacements.into_iter().rev() {
        lines.splice(replacement.start..replacement.end, replacement.new_lines);
    }

    Ok(lines.join(line_ending))
}

#[derive(Debug, Clone)]
struct PlannedReplacement {
    start: usize,
    end: usize,
    new_lines: Vec<String>,
}

#[derive(Debug, Clone)]
struct ResolvedSegments {
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

fn plan_replacements(
    original_lines: &[String],
    hunks: &[UpdateChunk],
) -> anyhow::Result<Vec<PlannedReplacement>> {
    let mut replacements = Vec::with_capacity(hunks.len());
    let mut working_lines = original_lines.to_vec();
    let mut search_start = 0;
    let mut line_offset = 0isize;

    for hunk in hunks {
        let start = resolve_hunk_start(&working_lines, hunk, search_start)?;
        let segments = resolve_segments(&working_lines, hunk, start)?;
        let start_idx = resolve_replacement_start(&working_lines, hunk, start, &segments)?;
        let original_start = translate_to_original_index(start_idx, line_offset)?;
        let end_idx = start_idx + segments.old_lines.len();

        replacements.push(PlannedReplacement {
            start: original_start,
            end: original_start + segments.old_lines.len(),
            new_lines: segments.new_lines.clone(),
        });

        working_lines.splice(start_idx..end_idx, segments.new_lines.clone());
        line_offset += segments.new_lines.len() as isize - segments.old_lines.len() as isize;
        search_start = next_search_start(start_idx, &segments.old_lines, &segments.new_lines, hunk)
            .min(working_lines.len());
    }

    Ok(replacements)
}

fn split_current_lines(current: &str) -> Vec<String> {
    if current.is_empty() {
        Vec::new()
    } else {
        current
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect()
    }
}

fn resolve_hunk_start(
    lines: &[String],
    hunk: &UpdateChunk,
    search_start: usize,
) -> anyhow::Result<usize> {
    match hunk.change_context.as_ref() {
        Some(ctx) => {
            let search_start = search_start.min(lines.len());

            let found = matcher::seek_sequence(lines, &[ctx.clone()], search_start, false);
            found.ok_or_else(|| anyhow::anyhow!("context line `{ctx}` not found"))
        }
        None => Ok(search_start.min(lines.len())),
    }
}

fn resolve_segments(
    lines: &[String],
    hunk: &UpdateChunk,
    start: usize,
) -> anyhow::Result<ResolvedSegments> {
    let initial = ResolvedSegments {
        old_lines: hunk.old_lines.clone(),
        new_lines: hunk.new_lines.clone(),
    };

    if initial.old_lines.is_empty() {
        return Ok(initial);
    }

    if seek_hunk_sequence(lines, &initial.old_lines, start, hunk.is_end_of_file).is_some() {
        return Ok(initial);
    }

    if hunk.is_end_of_file {
        if let Some(retry) = strip_trailing_empty_sentinel(&initial) {
            if retry.old_lines.is_empty()
                || seek_hunk_sequence(lines, &retry.old_lines, start, true).is_some()
            {
                return Ok(retry);
            }
        }
    }

    anyhow::bail!(
        "failed to find hunk lines `{}`",
        initial.old_lines.join("\n")
    )
}

fn strip_trailing_empty_sentinel(segments: &ResolvedSegments) -> Option<ResolvedSegments> {
    (segments.old_lines.len() > 1
        && matches!(segments.old_lines.last(), Some(last) if last.is_empty()))
    .then(|| ResolvedSegments {
        old_lines: segments.old_lines[..segments.old_lines.len() - 1].to_vec(),
        new_lines: strip_optional_trailing_empty(&segments.new_lines),
    })
}

fn strip_optional_trailing_empty(lines: &[String]) -> Vec<String> {
    if matches!(lines.last(), Some(last) if last.is_empty()) {
        lines[..lines.len() - 1].to_vec()
    } else {
        lines.to_vec()
    }
}

fn resolve_replacement_start(
    lines: &[String],
    hunk: &UpdateChunk,
    start: usize,
    segments: &ResolvedSegments,
) -> anyhow::Result<usize> {
    if segments.old_lines.is_empty() {
        return Ok(if !hunk.is_end_of_file && hunk.change_context.is_some() {
            start.min(lines.len())
        } else {
            eof_insert_index(lines)
        });
    }

    seek_hunk_sequence(lines, &segments.old_lines, start, hunk.is_end_of_file).ok_or_else(|| {
        anyhow::anyhow!(
            "failed to find hunk lines `{}`",
            segments.old_lines.join("\n")
        )
    })
}

fn seek_hunk_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    is_end_of_file: bool,
) -> Option<usize> {
    if !is_end_of_file {
        return matcher::seek_sequence(lines, pattern, start, false);
    }

    for (eof_start, search_len) in exact_eof_match_candidates(lines, pattern) {
        if start > eof_start {
            continue;
        }

        if let Some(idx) = matcher::seek_sequence(&lines[..search_len], pattern, eof_start, true)
            .filter(|&idx| idx == eof_start)
        {
            return Some(idx);
        }
    }

    None
}

fn exact_eof_match_candidates(lines: &[String], pattern: &[String]) -> Vec<(usize, usize)> {
    let mut candidates = Vec::with_capacity(2);
    let content_len = logical_content_len(lines);

    if let Some(start) = content_len.checked_sub(pattern.len()) {
        candidates.push((start, content_len));
    }

    if has_trailing_eof_sentinel(lines)
        && pattern.len() > 1
        && has_trailing_eof_sentinel(pattern)
        && let Some(start) = lines.len().checked_sub(pattern.len())
        && !candidates.contains(&(start, lines.len()))
    {
        candidates.push((start, lines.len()));
    }

    candidates
}

fn has_trailing_eof_sentinel(lines: &[String]) -> bool {
    matches!(lines.last(), Some(last) if last.is_empty())
}

fn logical_content_len(lines: &[String]) -> usize {
    if has_trailing_eof_sentinel(lines) {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    }
}

fn eof_insert_index(lines: &[String]) -> usize {
    match lines.last() {
        Some(last) if last.is_empty() => lines.len().saturating_sub(1),
        _ => lines.len(),
    }
}

fn translate_to_original_index(current_index: usize, line_offset: isize) -> anyhow::Result<usize> {
    let original_index = current_index as isize - line_offset;
    anyhow::ensure!(
        original_index >= 0,
        "planned replacement index became negative"
    );
    Ok(original_index as usize)
}

fn next_search_start(
    start_idx: usize,
    old_lines: &[String],
    new_lines: &[String],
    hunk: &UpdateChunk,
) -> usize {
    if old_lines.is_empty() && hunk.change_context.is_some() && !hunk.is_end_of_file {
        start_idx + new_lines.len() + 1
    } else {
        start_idx + new_lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::apply_hunks;
    use crate::patch::parser::UpdateChunk;

    fn chunk(
        change_context: Option<&str>,
        old_lines: &[&str],
        new_lines: &[&str],
        is_end_of_file: bool,
    ) -> UpdateChunk {
        UpdateChunk {
            change_context: change_context.map(str::to_string),
            old_lines: old_lines.iter().map(|line| (*line).to_string()).collect(),
            new_lines: new_lines.iter().map(|line| (*line).to_string()).collect(),
            is_end_of_file,
        }
    }

    #[test]
    fn pure_addition_chunks_append_at_end_like_codex() {
        let current = "alpha\nbeta\n";
        let hunks = vec![chunk(None, &[], &["gamma"], false)];

        assert_eq!(
            apply_hunks(current, &hunks, "\n").unwrap(),
            "alpha\nbeta\ngamma\n"
        );
    }

    #[test]
    fn eof_match_retries_without_trailing_empty_sentinel() {
        let current = "alpha\nbeta";
        let hunks = vec![chunk(None, &["beta", ""], &["beta", "gamma", ""], true)];

        assert_eq!(
            apply_hunks(current, &hunks, "\n").unwrap(),
            "alpha\nbeta\ngamma"
        );
    }

    #[test]
    fn eof_retry_does_not_match_non_terminal_occurrence() {
        let current = "beta\nomega\ntail";
        let hunks = vec![chunk(None, &["beta", ""], &["beta", "gamma", ""], true)];

        let err = apply_hunks(current, &hunks, "\n").unwrap_err();

        assert!(err.to_string().contains("failed to find hunk lines"));
    }

    #[test]
    fn eof_hunk_matches_last_real_line_in_newline_terminated_file() {
        let current = "before\nmiddle\nbefore\n";
        let hunks = vec![chunk(None, &["before"], &["after"], true)];

        assert_eq!(
            apply_hunks(current, &hunks, "\n").unwrap(),
            "before\nmiddle\nafter\n"
        );
    }

    #[test]
    fn eof_hunk_replaces_blank_last_real_line_in_newline_terminated_file() {
        let current = "alpha\n\n";
        let hunks = vec![chunk(None, &[""], &["omega"], true)];

        assert_eq!(
            apply_hunks(current, &hunks, "\n").unwrap(),
            "alpha\nomega\n"
        );
    }

    #[test]
    fn change_context_matches_with_trailing_whitespace() {
        let current = "alpha\nmarker  \ntail\n";
        let hunks = vec![chunk(Some("marker"), &[], &["inserted"], false)];

        assert_eq!(
            apply_hunks(current, &hunks, "\n").unwrap(),
            "alpha\ninserted\nmarker  \ntail\n"
        );
    }

    #[test]
    fn crlf_input_round_trips_as_crlf() {
        let current = "alpha\r\nbeta\r\n";
        let hunks = vec![chunk(None, &["alpha"], &["omega"], false)];

        assert_eq!(
            apply_hunks(current, &hunks, "\r\n").unwrap(),
            "omega\r\nbeta\r\n"
        );
    }

    #[test]
    fn singleton_empty_eof_hunk_does_not_retry_as_pure_insertion() {
        let current = "alpha";
        let hunks = vec![chunk(None, &[""], &["omega"], true)];

        let err = apply_hunks(current, &hunks, "\n").unwrap_err();

        assert!(err.to_string().contains("failed to find hunk lines"));
    }
}
