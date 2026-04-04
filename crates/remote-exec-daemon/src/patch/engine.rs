use super::parser::{Hunk, HunkLine};

pub fn apply_hunks(current: &str, hunks: &[Hunk]) -> anyhow::Result<String> {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();
    let mut search_start = 0;

    for hunk in hunks {
        let (old_lines, new_lines) = build_segments(hunk);
        let start = resolve_hunk_start(&lines, hunk, search_start)?;

        let start_idx = if old_lines.is_empty() {
            if hunk.end_of_file {
                lines.len()
            } else {
                start.min(lines.len())
            }
        } else {
            seek_sequence(&lines, &old_lines, start, hunk.end_of_file).ok_or_else(|| {
                anyhow::anyhow!("failed to find hunk lines `{}`", old_lines.join("\n"))
            })?
        };

        let next_search_start = next_search_start(start_idx, &old_lines, &new_lines, hunk);
        lines.splice(start_idx..start_idx + old_lines.len(), new_lines);
        search_start = next_search_start.min(lines.len());
    }

    Ok(lines.join("\n"))
}

fn resolve_hunk_start(lines: &[String], hunk: &Hunk, search_start: usize) -> anyhow::Result<usize> {
    match hunk.context.as_ref() {
        Some(ctx) => {
            let found = if hunk.end_of_file {
                lines[search_start.min(lines.len())..]
                    .iter()
                    .rposition(|line| line == ctx)
                    .map(|idx| idx + search_start.min(lines.len()))
            } else {
                lines[search_start.min(lines.len())..]
                    .iter()
                    .position(|line| line == ctx)
                    .map(|idx| idx + search_start.min(lines.len()))
            };
            found.ok_or_else(|| anyhow::anyhow!("context line `{ctx}` not found"))
        }
        None => Ok(search_start.min(lines.len())),
    }
}

fn build_segments(hunk: &Hunk) -> (Vec<String>, Vec<String>) {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    for line in &hunk.lines {
        match line {
            HunkLine::Context(value) => {
                old_lines.push(value.clone());
                new_lines.push(value.clone());
            }
            HunkLine::Delete(value) => old_lines.push(value.clone()),
            HunkLine::Add(value) => new_lines.push(value.clone()),
        }
    }

    (old_lines, new_lines)
}

fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    end_of_file: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start.min(lines.len()));
    }

    if pattern.len() > lines.len() {
        return None;
    }

    let max_start = lines.len() - pattern.len();
    if end_of_file {
        let eof_start = max_start;
        return (eof_start >= start && lines[eof_start..eof_start + pattern.len()] == *pattern)
            .then_some(eof_start);
    }

    (start..=max_start).find(|&idx| lines[idx..idx + pattern.len()] == *pattern)
}

fn next_search_start(
    start_idx: usize,
    old_lines: &[String],
    new_lines: &[String],
    hunk: &Hunk,
) -> usize {
    if old_lines.is_empty() && hunk.context.is_some() && !hunk.end_of_file {
        start_idx + new_lines.len() + 1
    } else {
        start_idx + new_lines.len()
    }
}
