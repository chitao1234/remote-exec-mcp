use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchAction {
    Add {
        path: PathBuf,
        lines: Vec<String>,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        move_to: Option<PathBuf>,
        hunks: Vec<Hunk>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub context: Option<String>,
    pub lines: Vec<HunkLine>,
    pub end_of_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String),
    Delete(String),
    Add(String),
}

fn is_horizontal_whitespace(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

fn trim_horizontal(line: &str) -> &str {
    line.trim_matches(is_horizontal_whitespace)
}

fn strip_control_prefix<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    trim_horizontal(line)
        .strip_prefix(prefix)
        .map(|rest| rest.trim_matches(is_horizontal_whitespace))
}

fn is_structural_control_line(line: &str) -> bool {
    trim_horizontal(line).starts_with("*** ")
}

fn parse_hunk_header(line: &str) -> anyhow::Result<Option<String>> {
    let line = trim_horizontal(line);
    if line == "@@" {
        return Ok(None);
    }

    if let Some(rest) = line.strip_prefix("@@ ") {
        return Ok(Some(rest.to_string()));
    }

    anyhow::bail!("invalid update hunk header `{line}`");
}

pub fn parse_patch(input: &str) -> anyhow::Result<Vec<PatchAction>> {
    let lines: Vec<&str> = input.lines().collect();
    anyhow::ensure!(
        lines.first().copied().map(trim_horizontal) == Some("*** Begin Patch"),
        "invalid patch header"
    );
    anyhow::ensure!(
        lines.last().copied().map(trim_horizontal) == Some("*** End Patch"),
        "invalid patch footer"
    );

    let mut actions = Vec::new();
    let mut index = 1;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = strip_control_prefix(line, "*** Add File: ") {
            index += 1;
            let mut added = Vec::new();
            while index + 1 < lines.len() && !is_structural_control_line(lines[index]) {
                let raw = lines[index];
                let value = raw
                    .strip_prefix('+')
                    .ok_or_else(|| anyhow::anyhow!("add file lines must start with `+`"))?;
                added.push(value.to_string());
                index += 1;
            }
            actions.push(PatchAction::Add {
                path: path.into(),
                lines: added,
            });
            continue;
        }

        if let Some(path) = strip_control_prefix(line, "*** Delete File: ") {
            actions.push(PatchAction::Delete { path: path.into() });
            index += 1;
            continue;
        }

        if let Some(path) = strip_control_prefix(line, "*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index + 1 < lines.len()
                && let Some(destination) = strip_control_prefix(lines[index], "*** Move to: ")
            {
                move_to = Some(destination.into());
                index += 1;
            }

            let mut hunks = Vec::new();
            while index + 1 < lines.len() && !is_structural_control_line(lines[index]) {
                let context = parse_hunk_header(lines[index])?;
                index += 1;

                let mut hunk_lines = Vec::new();
                while index + 1 < lines.len()
                    && !trim_horizontal(lines[index]).starts_with("@@")
                    && trim_horizontal(lines[index]) != "*** End of File"
                    && !is_structural_control_line(lines[index])
                {
                    let raw = lines[index];
                    let parsed = match raw.chars().next() {
                        Some(' ') => HunkLine::Context(raw[1..].to_string()),
                        Some('-') => HunkLine::Delete(raw[1..].to_string()),
                        Some('+') => HunkLine::Add(raw[1..].to_string()),
                        _ => anyhow::bail!("invalid update hunk line `{raw}`"),
                    };
                    hunk_lines.push(parsed);
                    index += 1;
                }
                let end_of_file =
                    if index + 1 < lines.len() && trim_horizontal(lines[index]) == "*** End of File"
                    {
                        index += 1;
                        true
                    } else {
                        false
                    };
                anyhow::ensure!(!hunk_lines.is_empty(), "update hunk with no changes");
                hunks.push(Hunk {
                    context,
                    lines: hunk_lines,
                    end_of_file,
                });
            }

            actions.push(PatchAction::Update {
                path: path.into(),
                move_to,
                hunks,
            });
            continue;
        }

        anyhow::bail!("unsupported patch line `{}`", trim_horizontal(line));
    }

    anyhow::ensure!(!actions.is_empty(), "empty patch");
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::{Hunk, HunkLine, PatchAction, parse_patch};

    #[test]
    fn parses_control_lines_with_horizontal_whitespace() {
        let patch = concat!(
            " \t*** Begin Patch\t\n",
            "\t*** Update File: old.txt  \n",
            "  *** Move to: new.txt\t\n",
            " \t@@\t\n",
            "-old\n",
            "+new\n",
            "\t*** End of File \n",
            "  *** End Patch\t\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap(),
            vec![PatchAction::Update {
                path: "old.txt".into(),
                move_to: Some("new.txt".into()),
                hunks: vec![Hunk {
                    context: None,
                    lines: vec![
                        HunkLine::Delete("old".to_string()),
                        HunkLine::Add("new".to_string()),
                    ],
                    end_of_file: true,
                }],
            }]
        );
    }
}
