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

pub fn parse_patch(input: &str) -> anyhow::Result<Vec<PatchAction>> {
    let lines: Vec<&str> = input.lines().collect();
    anyhow::ensure!(
        lines.first() == Some(&"*** Begin Patch"),
        "invalid patch header"
    );
    anyhow::ensure!(
        lines.last() == Some(&"*** End Patch"),
        "invalid patch footer"
    );

    let mut actions = Vec::new();
    let mut index = 1;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut added = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
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

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            actions.push(PatchAction::Delete { path: path.into() });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index + 1 < lines.len()
                && let Some(destination) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(destination.into());
                index += 1;
            }

            let mut hunks = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                let header = lines[index];
                let context = if header == "@@" {
                    None
                } else if let Some(rest) = header.strip_prefix("@@ ") {
                    Some(rest.to_string())
                } else {
                    anyhow::bail!("invalid update hunk header `{header}`");
                };
                index += 1;

                let mut hunk_lines = Vec::new();
                while index + 1 < lines.len()
                    && !lines[index].starts_with("@@")
                    && !lines[index].starts_with("*** End of File")
                    && !lines[index].starts_with("*** ")
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
                let end_of_file = if index + 1 < lines.len() && lines[index] == "*** End of File" {
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

        anyhow::bail!("unsupported patch line `{line}`");
    }

    anyhow::ensure!(!actions.is_empty(), "empty patch");
    Ok(actions)
}
