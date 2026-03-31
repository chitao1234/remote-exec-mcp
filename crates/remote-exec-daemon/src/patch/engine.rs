use super::parser::{Hunk, HunkLine};

pub fn apply_hunks(current: &str, hunks: &[Hunk]) -> anyhow::Result<String> {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();

    for hunk in hunks {
        let (old_lines, new_lines) = build_segments(hunk);
        let start = hunk
            .context
            .as_ref()
            .and_then(|ctx| {
                if hunk.end_of_file {
                    lines.iter().rposition(|line| line == ctx)
                } else {
                    lines.iter().position(|line| line == ctx)
                }
            })
            .unwrap_or(0);

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

        lines.splice(start_idx..start_idx + old_lines.len(), new_lines);
    }

    Ok(lines.join("\n"))
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

    for idx in start..=max_start {
        if lines[idx..idx + pattern.len()] == *pattern {
            return Some(idx);
        }
    }

    None
}
