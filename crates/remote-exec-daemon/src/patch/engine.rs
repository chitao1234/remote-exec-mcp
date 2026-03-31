use super::parser::{Hunk, HunkLine};

pub fn apply_hunks(current: &str, hunks: &[Hunk]) -> anyhow::Result<String> {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();

    for hunk in hunks {
        let anchor = hunk
            .context
            .as_ref()
            .and_then(|ctx| lines.iter().position(|line| line == ctx));
        let mut cursor = anchor.unwrap_or(0);
        let mut replacement = Vec::new();

        for line in &hunk.lines {
            match line {
                HunkLine::Context(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("context line `{value}` not found"))?;
                    replacement.extend(lines[cursor..=found].iter().cloned());
                    cursor = found + 1;
                }
                HunkLine::Delete(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("delete line `{value}` not found"))?;
                    replacement.extend(lines[cursor..found].iter().cloned());
                    cursor = found + 1;
                }
                HunkLine::Add(value) => replacement.push(value.clone()),
            }
        }

        replacement.extend(lines[cursor..].iter().cloned());
        lines = replacement;
    }

    Ok(lines.join("\n"))
}
