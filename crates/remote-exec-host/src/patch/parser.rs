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
    PatchParser::new(input)?.parse_actions()
}

struct PatchParser<'a> {
    lines: Vec<&'a str>,
    index: usize,
}

impl<'a> PatchParser<'a> {
    fn new(input: &'a str) -> anyhow::Result<Self> {
        let lines: Vec<&str> = input.lines().collect();
        anyhow::ensure!(
            lines.first().copied().map(trim_horizontal) == Some("*** Begin Patch"),
            "invalid patch header"
        );
        anyhow::ensure!(
            lines.last().copied().map(trim_horizontal) == Some("*** End Patch"),
            "invalid patch footer"
        );

        Ok(Self { lines, index: 1 })
    }

    fn parse_actions(&mut self) -> anyhow::Result<Vec<PatchAction>> {
        let mut actions = Vec::new();
        while !self.at_body_end() {
            actions.push(self.parse_action()?);
        }

        anyhow::ensure!(!actions.is_empty(), "empty patch");
        Ok(actions)
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

        anyhow::bail!("unsupported patch line `{}`", trim_horizontal(line));
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
            hunks.push(self.parse_update_chunk(&path, hunks.is_empty())?);
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
        if trim_horizontal(line).starts_with("@@") {
            let header = parse_hunk_header(line)?;
            self.advance();
            return Ok(header);
        }

        if is_first_hunk {
            return Ok(None);
        }

        anyhow::bail!("invalid update hunk header `{}`", trim_horizontal(line));
    }

    fn current_line_is_update_chunk_body(&self) -> bool {
        !self.at_body_end()
            && !trim_horizontal(self.current()).starts_with("@@")
            && trim_horizontal(self.current()) != "*** End of File"
            && !is_structural_control_line(self.current())
    }

    fn consume_end_of_file_marker(&mut self) -> bool {
        if !self.at_body_end() && trim_horizontal(self.current()) == "*** End of File" {
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
