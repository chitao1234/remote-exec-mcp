#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptedApplyPatch {
    pub patch: String,
    pub workdir: Option<String>,
}

pub fn maybe_intercept_apply_patch(
    cmd: &str,
    workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    let patch = parse_direct_invocation(cmd.trim())?;
    Some(InterceptedApplyPatch {
        patch,
        workdir: workdir.map(ToString::to_string),
    })
}

fn parse_direct_invocation(cmd: &str) -> Option<String> {
    ["apply_patch", "applypatch"]
        .into_iter()
        .find_map(|name| {
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
}
