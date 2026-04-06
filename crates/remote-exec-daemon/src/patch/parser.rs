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
        hunks: Vec<UpdateChunk>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateChunk {
    pub change_context: Option<String>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub is_end_of_file: bool,
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

fn parse_update_chunk_line(
    line: &str,
    old_lines: &mut Vec<String>,
    new_lines: &mut Vec<String>,
) -> anyhow::Result<()> {
    match line.chars().next() {
        Some(' ') => {
            let value = line[1..].to_string();
            old_lines.push(value.clone());
            new_lines.push(value);
        }
        Some('-') => old_lines.push(line[1..].to_string()),
        Some('+') => new_lines.push(line[1..].to_string()),
        _ => anyhow::bail!("invalid update hunk line `{line}`"),
    }

    Ok(())
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
            let path = PathBuf::from(path);
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
                let change_context = if trim_horizontal(lines[index]).starts_with("@@") {
                    let header = parse_hunk_header(lines[index])?;
                    index += 1;
                    header
                } else if hunks.is_empty() {
                    None
                } else {
                    anyhow::bail!(
                        "invalid update hunk header `{}`",
                        trim_horizontal(lines[index])
                    );
                };

                let mut old_lines = Vec::new();
                let mut new_lines = Vec::new();
                while index + 1 < lines.len()
                    && !trim_horizontal(lines[index]).starts_with("@@")
                    && trim_horizontal(lines[index]) != "*** End of File"
                    && !is_structural_control_line(lines[index])
                {
                    parse_update_chunk_line(lines[index], &mut old_lines, &mut new_lines)?;
                    index += 1;
                }
                let is_end_of_file = if index + 1 < lines.len()
                    && trim_horizontal(lines[index]) == "*** End of File"
                {
                    index += 1;
                    true
                } else {
                    false
                };
                anyhow::ensure!(
                    !old_lines.is_empty() || !new_lines.is_empty(),
                    "update hunk for path `{}` has no changes",
                    path.display()
                );
                hunks.push(UpdateChunk {
                    change_context,
                    old_lines,
                    new_lines,
                    is_end_of_file,
                });
            }
            anyhow::ensure!(
                !hunks.is_empty(),
                "update file hunk for path `{}` is empty",
                path.display()
            );

            actions.push(PatchAction::Update {
                path,
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
    use super::{PatchAction, UpdateChunk, parse_patch};

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
                hunks: vec![UpdateChunk {
                    change_context: None,
                    old_lines: vec!["old".to_string()],
                    new_lines: vec!["new".to_string()],
                    is_end_of_file: true,
                }],
            }]
        );
    }

    #[test]
    fn parses_first_update_chunk_without_explicit_header() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: demo.txt\n",
            " line1\n",
            "+line2\n",
            "*** End Patch\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap(),
            vec![PatchAction::Update {
                path: "demo.txt".into(),
                move_to: None,
                hunks: vec![UpdateChunk {
                    change_context: None,
                    old_lines: vec!["line1".to_string()],
                    new_lines: vec!["line1".to_string(), "line2".to_string()],
                    is_end_of_file: false,
                }],
            }]
        );
    }

    #[test]
    fn rejects_empty_update_file_sections() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: demo.txt\n",
            "*** End Patch\n",
        );

        let err = parse_patch(patch).unwrap_err();
        assert!(
            err.to_string()
                .contains("update file hunk for path `demo.txt` is empty"),
            "{err}"
        );
    }
}
