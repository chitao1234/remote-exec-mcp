use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptedApplyPatch {
    pub patch: String,
    pub workdir: Option<String>,
}

fn is_horizontal_whitespace(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

fn trim_horizontal_start(text: &str) -> &str {
    text.trim_start_matches(is_horizontal_whitespace)
}

pub fn maybe_intercept_apply_patch(
    cmd: &str,
    workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    let trimmed = cmd.trim();
    if let Some(patch) = parse_direct_invocation(trimmed) {
        return Some(InterceptedApplyPatch {
            patch,
            workdir: workdir.map(ToString::to_string),
        });
    }

    let (effective_workdir, script) = split_cd_wrapper(trimmed, workdir);
    let (command_name, body) = parse_heredoc_invocation(script)?;
    if command_name != "apply_patch" && command_name != "applypatch" {
        return None;
    }

    Some(InterceptedApplyPatch {
        patch: format!("{body}\n"),
        workdir: effective_workdir,
    })
}

fn parse_direct_invocation(cmd: &str) -> Option<String> {
    ["apply_patch", "applypatch"].into_iter().find_map(|name| {
        let rest = cmd.strip_prefix(name)?.trim_start();
        parse_single_quoted_argument(rest)
    })
}

fn parse_single_quoted_argument(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let end = rest[1..].find(quote)? + 1;
    let patch = &rest[1..end];
    let trailing = &rest[end + 1..];
    if !trailing.trim().is_empty() {
        return None;
    }

    Some(patch.to_string())
}

fn split_cd_wrapper<'a>(cmd: &'a str, workdir: Option<&str>) -> (Option<String>, &'a str) {
    if let Some(rest) = cmd.strip_prefix("cd") {
        let Some(first) = rest.chars().next() else {
            return (workdir.map(ToString::to_string), cmd);
        };
        if !is_horizontal_whitespace(first) {
            return (workdir.map(ToString::to_string), cmd);
        }

        let rest = trim_horizontal_start(rest);
        if let Some((path, tail)) = rest.split_once("&&") {
            let path = path.trim_matches(is_horizontal_whitespace);
            if path.is_empty() || path.chars().any(char::is_whitespace) {
                return (workdir.map(ToString::to_string), cmd);
            }

            let mut resolved = workdir.map(PathBuf::from).unwrap_or_default();
            resolved.push(path);
            return (
                Some(resolved.display().to_string()),
                trim_horizontal_start(tail),
            );
        }
    }

    (workdir.map(ToString::to_string), cmd)
}

fn parse_heredoc_invocation(cmd: &str) -> Option<(&str, &str)> {
    let operator = cmd.find("<<")?;
    let command_name = cmd[..operator].trim();
    let mut rest = &cmd[operator + 2..];
    rest = trim_horizontal_start(rest);

    let rest = rest.strip_prefix('\'')?;
    let delimiter_end = rest.find('\'')?;
    let delimiter = &rest[..delimiter_end];
    let body_with_newline = rest[delimiter_end + 1..].strip_prefix('\n')?;
    let marker = format!("\n{delimiter}");
    let (body, trailing) = body_with_newline.rsplit_once(&marker)?;
    if !trailing.trim().is_empty() {
        return None;
    }

    Some((command_name, body))
}

#[cfg(test)]
mod tests {
    use super::{InterceptedApplyPatch, maybe_intercept_apply_patch};

    #[test]
    fn parses_direct_apply_patch_command() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: hello.txt\n",
            "+hello\n",
            "*** End Patch\n",
        );
        let cmd = format!("apply_patch '{patch}'");

        assert_eq!(
            maybe_intercept_apply_patch(&cmd, Some("workspace")),
            Some(InterceptedApplyPatch {
                patch: patch.to_string(),
                workdir: Some("workspace".to_string()),
            })
        );
    }

    #[test]
    fn parses_direct_applypatch_alias() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: alias.txt\n",
            "+alias\n",
            "*** End Patch\n",
        );
        let cmd = format!("applypatch \"{patch}\"");

        assert_eq!(
            maybe_intercept_apply_patch(&cmd, None),
            Some(InterceptedApplyPatch {
                patch: patch.to_string(),
                workdir: None,
            })
        );
    }

    #[test]
    fn rejects_raw_patch_body_and_extra_commands() {
        let raw_patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: no.txt\n",
            "+no\n",
            "*** End Patch\n",
        );

        assert_eq!(maybe_intercept_apply_patch(raw_patch, None), None);
        assert_eq!(
            maybe_intercept_apply_patch(&format!("apply_patch '{raw_patch}' && echo done"), None),
            None
        );
    }

    #[test]
    fn parses_applypatch_heredoc_with_cd_wrapper_relative_to_workdir() {
        let cmd = concat!(
            "cd nested && applypatch <<'PATCH'\n",
            "*** Begin Patch\n",
            "*** Add File: hello.txt\n",
            "+hello\n",
            "*** End Patch\n",
            "PATCH\n",
        );

        assert_eq!(
            maybe_intercept_apply_patch(cmd, Some("outer")),
            Some(InterceptedApplyPatch {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Add File: hello.txt\n",
                    "+hello\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some("outer/nested".to_string()),
            })
        );
    }

    #[test]
    fn parses_apply_patch_invocations_with_horizontal_whitespace() {
        let direct_patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: direct.txt\n",
            "+direct\n",
            "*** End Patch\n",
        );
        let direct_cmd = format!(" \tapply_patch\t  '{direct_patch}' \t");

        assert_eq!(
            maybe_intercept_apply_patch(&direct_cmd, Some("workspace")),
            Some(InterceptedApplyPatch {
                patch: direct_patch.to_string(),
                workdir: Some("workspace".to_string()),
            })
        );

        let heredoc_cmd = concat!(
            "cd\t nested  && \tapplypatch\t <<'PATCH'\n",
            "*** Begin Patch\n",
            "*** Add File: heredoc.txt\n",
            "+heredoc\n",
            "*** End Patch\n",
            "PATCH\n",
        );

        assert_eq!(
            maybe_intercept_apply_patch(heredoc_cmd, Some("outer")),
            Some(InterceptedApplyPatch {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Add File: heredoc.txt\n",
                    "+heredoc\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some("outer/nested".to_string()),
            })
        );
    }
}
