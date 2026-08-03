use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPatch {
    pub actions: Vec<PatchAction>,
    pub environment_id: Option<String>,
}

fn trim_patch_whitespace(line: &str) -> &str {
    line.trim()
}

fn strip_control_prefix<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    trim_patch_whitespace(line)
        .strip_prefix(prefix)
        .map(str::trim)
}

fn is_structural_control_line(line: &str) -> bool {
    trim_patch_whitespace(line).starts_with("*** ")
}

fn parse_hunk_header(line: &str) -> anyhow::Result<Option<String>> {
    let line = trim_patch_whitespace(line);
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
        None => {
            old_lines.push(String::new());
            new_lines.push(String::new());
        }
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

pub fn parse_patch(input: &str) -> anyhow::Result<ParsedPatch> {
    PatchParser::new(input)?.parse_actions()
}

struct PatchParser<'a> {
    lines: Vec<&'a str>,
    index: usize,
    environment_id: Option<String>,
}

impl<'a> PatchParser<'a> {
    fn new(input: &'a str) -> anyhow::Result<Self> {
        let lines: Vec<&str> = input.lines().collect();
        anyhow::ensure!(
            lines.first().copied().map(trim_patch_whitespace) == Some("*** Begin Patch"),
            "invalid patch header"
        );
        anyhow::ensure!(
            lines.last().copied().map(trim_patch_whitespace) == Some("*** End Patch"),
            "invalid patch footer"
        );

        Ok(Self {
            lines,
            index: 1,
            environment_id: None,
        })
    }

    fn parse_actions(&mut self) -> anyhow::Result<ParsedPatch> {
        self.parse_environment_id()?;
        let mut actions = Vec::new();
        while !self.at_body_end() {
            actions.push(self.parse_action()?);
        }

        Ok(ParsedPatch {
            actions,
            environment_id: self.environment_id.take(),
        })
    }

    fn parse_environment_id(&mut self) -> anyhow::Result<()> {
        if self.at_body_end() {
            return Ok(());
        }

        let Some(environment_id) = strip_control_prefix(self.current(), "*** Environment ID:")
        else {
            return Ok(());
        };
        anyhow::ensure!(
            !environment_id.is_empty(),
            "patch environment ID cannot be empty"
        );
        self.environment_id = Some(environment_id.to_string());
        self.advance();
        Ok(())
    }

    fn parse_action(&mut self) -> anyhow::Result<PatchAction> {
        let line = self.current();
        if let Some(path) = strip_control_prefix(line, "*** Add File: ") {
            return self.parse_add_file(path.into());
        }

        if let Some(path) = strip_control_prefix(line, "*** Delete File: ") {
            return Ok(self.parse_delete_file(path.into()));
        }

        if let Some(path) = strip_control_prefix(line, "*** Update File: ") {
            return self.parse_update_file(path.into());
        }

        anyhow::bail!("unsupported patch line `{}`", trim_patch_whitespace(line));
    }

    fn parse_add_file(&mut self, path: PathBuf) -> anyhow::Result<PatchAction> {
        self.advance();
        let mut added = Vec::new();
        while !self.at_body_end() && !is_structural_control_line(self.current()) {
            let raw = self.current();
            let value = raw
                .strip_prefix('+')
                .ok_or_else(|| anyhow::anyhow!("add file lines must start with `+`"))?;
            added.push(value.to_string());
            self.advance();
        }

        Ok(PatchAction::Add { path, lines: added })
    }

    fn parse_delete_file(&mut self, path: PathBuf) -> PatchAction {
        self.advance();
        PatchAction::Delete { path }
    }

    fn parse_update_file(&mut self, path: PathBuf) -> anyhow::Result<PatchAction> {
        self.advance();
        let move_to = self.parse_move_to();
        let mut hunks = Vec::new();
        while !self.at_body_end() && !is_structural_control_line(self.current()) {
            let chunk = self.parse_update_chunk(&path, hunks.is_empty())?;
            let is_end_of_file = chunk.is_end_of_file;
            hunks.push(chunk);
            if is_end_of_file {
                self.consume_blank_lines_after_end_of_file();
            }
        }
        anyhow::ensure!(
            !hunks.is_empty(),
            "update file hunk for path `{}` is empty",
            path.display()
        );

        Ok(PatchAction::Update {
            path,
            move_to,
            hunks,
        })
    }

    fn parse_move_to(&mut self) -> Option<PathBuf> {
        if self.at_body_end() {
            return None;
        }

        let destination = strip_control_prefix(self.current(), "*** Move to: ")?;
        let destination = PathBuf::from(destination);
        self.advance();
        Some(destination)
    }

    fn parse_update_chunk(
        &mut self,
        path: &Path,
        is_first_hunk: bool,
    ) -> anyhow::Result<UpdateChunk> {
        let change_context = self.parse_update_chunk_header(is_first_hunk)?;
        let mut old_lines = Vec::new();
        let mut new_lines = Vec::new();
        while self.current_line_is_update_chunk_body() {
            parse_update_chunk_line(self.current(), &mut old_lines, &mut new_lines)?;
            self.advance();
        }

        let is_end_of_file = self.consume_end_of_file_marker();
        anyhow::ensure!(
            !old_lines.is_empty() || !new_lines.is_empty(),
            "update hunk for path `{}` has no changes",
            path.display()
        );

        Ok(UpdateChunk {
            change_context,
            old_lines,
            new_lines,
            is_end_of_file,
        })
    }

    fn parse_update_chunk_header(&mut self, is_first_hunk: bool) -> anyhow::Result<Option<String>> {
        let line = self.current();
        if trim_patch_whitespace(line).starts_with("@@") {
            let header = parse_hunk_header(line)?;
            self.advance();
            return Ok(header);
        }

        if is_first_hunk {
            return Ok(None);
        }

        anyhow::bail!(
            "invalid update hunk header `{}`",
            trim_patch_whitespace(line)
        );
    }

    fn current_line_is_update_chunk_body(&self) -> bool {
        !self.at_body_end()
            && !trim_patch_whitespace(self.current()).starts_with("@@")
            && trim_patch_whitespace(self.current()) != "*** End of File"
            && !is_structural_control_line(self.current())
    }

    fn consume_blank_lines_after_end_of_file(&mut self) {
        while !self.at_body_end() && trim_patch_whitespace(self.current()).is_empty() {
            self.advance();
        }
    }

    fn consume_end_of_file_marker(&mut self) -> bool {
        if !self.at_body_end() && trim_patch_whitespace(self.current()) == "*** End of File" {
            self.advance();
            true
        } else {
            false
        }
    }

    fn at_body_end(&self) -> bool {
        self.index + 1 >= self.lines.len()
    }

    fn current(&self) -> &'a str {
        self.lines[self.index]
    }

    fn advance(&mut self) {
        self.index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedPatch, PatchAction, UpdateChunk, parse_patch};

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
            parse_patch(patch).unwrap().actions,
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
            parse_patch(patch).unwrap().actions,
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

    #[test]
    fn accepts_empty_patch_envelope() {
        let patch = "*** Begin Patch\n*** End Patch\n";

        assert_eq!(parse_patch(patch).unwrap().actions, Vec::new());
    }

    #[test]
    fn parses_blank_context_line_in_update_hunk() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: demo.txt\n",
            " before\n",
            "\n",
            "-after\n",
            "+changed\n",
            "*** End Patch\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap().actions,
            vec![PatchAction::Update {
                path: "demo.txt".into(),
                move_to: None,
                hunks: vec![UpdateChunk {
                    change_context: None,
                    old_lines: vec!["before".to_string(), String::new(), "after".to_string(),],
                    new_lines: vec!["before".to_string(), String::new(), "changed".to_string(),],
                    is_end_of_file: false,
                }],
            }]
        );
    }

    #[test]
    fn accepts_blank_separator_after_end_of_file() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: demo.txt\n",
            "@@\n",
            " old\n",
            "+after\n",
            "*** End of File\n",
            "\n",
            "@@\n",
            "+final\n",
            "*** End Patch\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap().actions,
            vec![PatchAction::Update {
                path: "demo.txt".into(),
                move_to: None,
                hunks: vec![
                    UpdateChunk {
                        change_context: None,
                        old_lines: vec!["old".to_string()],
                        new_lines: vec!["old".to_string(), "after".to_string()],
                        is_end_of_file: true,
                    },
                    UpdateChunk {
                        change_context: None,
                        old_lines: Vec::new(),
                        new_lines: vec!["final".to_string()],
                        is_end_of_file: false,
                    },
                ],
            }]
        );
    }

    #[test]
    fn parses_control_lines_with_unicode_whitespace() {
        let patch = concat!(
            "\u{00a0}*** Begin Patch\u{3000}\n",
            "\u{2003}*** Add File: demo.txt\u{2002}\n",
            "+demo\n",
            "\u{202f}*** End Patch\u{1680}\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap().actions,
            vec![PatchAction::Add {
                path: "demo.txt".into(),
                lines: vec!["demo".to_string()],
            }]
        );
    }

    #[test]
    fn parses_environment_id_without_affecting_actions() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Environment ID: builder-a\n",
            "*** Add File: demo.txt\n",
            "+demo\n",
            "*** End Patch\n",
        );

        assert_eq!(
            parse_patch(patch).unwrap(),
            ParsedPatch {
                actions: vec![PatchAction::Add {
                    path: "demo.txt".into(),
                    lines: vec!["demo".to_string()],
                }],
                environment_id: Some("builder-a".to_string()),
            }
        );
    }

    #[test]
    fn rejects_empty_environment_id() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Environment ID: \n",
            "*** End Patch\n",
        );

        let err = parse_patch(patch).unwrap_err();
        assert!(
            err.to_string().contains("environment ID cannot be empty"),
            "{err}"
        );
    }
}
