# Apply Patch Whitespace Tolerance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make direct `apply_patch` and explicit `exec_command` interception accept horizontal whitespace variation while preserving the current narrow grammars and output shapes.

**Architecture:** Keep the existing split between daemon-side patch parsing and broker-side exec interception. Teach the daemon parser to normalize only structural patch lines, and teach the broker interception parser to accept extra spaces or tabs around the already-supported wrapper tokens without turning it into a general shell parser.

**Tech Stack:** Rust 2024, Tokio, Axum integration tests, rmcp broker tests, cargo test, cargo fmt, clippy

---

## File Map

- Modify: `crates/remote-exec-daemon/src/patch/parser.rs`
  Responsibility: accept leading and trailing horizontal whitespace on structural patch lines while keeping payload-line semantics strict.
- Modify: `crates/remote-exec-daemon/tests/patch_rpc.rs`
  Responsibility: prove the public patch RPC accepts tolerant control-line formatting.
- Modify: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
  Responsibility: accept horizontal whitespace around the existing direct and heredoc wrapper tokens without broadening the accepted grammar.
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
  Responsibility: prove tolerant wrapper forms still intercept, still avoid `/v1/exec/start`, and still preserve wrapped unified-exec output.

### Task 1: Add Horizontal-Whitespace Tolerance To Patch Control Lines

**Files:**
- Modify: `crates/remote-exec-daemon/src/patch/parser.rs`
- Modify: `crates/remote-exec-daemon/tests/patch_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon patch::parser::tests::parses_control_lines_with_horizontal_whitespace -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: the daemon parser has a tight unit-test seam for structural recognition, and the public patch RPC can then confirm the same behavior end to end.

- [ ] **Step 1: Add a failing parser unit test for tolerant control lines**

```rust
// crates/remote-exec-daemon/src/patch/parser.rs
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
```

- [ ] **Step 2: Run the focused parser verification and confirm it fails first**

Run: `cargo test -p remote-exec-daemon patch::parser::tests::parses_control_lines_with_horizontal_whitespace -- --exact --nocapture`
Expected: FAIL because `parse_patch(...)` still requires exact marker strings such as `*** Begin Patch` and `*** End Patch`.

- [ ] **Step 3: Implement horizontal-whitespace-tolerant structural parsing**

```rust
// crates/remote-exec-daemon/src/patch/parser.rs
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
        lines.first().map(|line| trim_horizontal(line)) == Some("*** Begin Patch"),
        "invalid patch header"
    );
    anyhow::ensure!(
        lines.last().map(|line| trim_horizontal(line)) == Some("*** End Patch"),
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
                && let Some(destination) =
                    strip_control_prefix(lines[index], "*** Move to: ")
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
                let end_of_file = if index + 1 < lines.len()
                    && trim_horizontal(lines[index]) == "*** End of File"
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
```

- [ ] **Step 4: Run the focused parser verification again**

Run: `cargo test -p remote-exec-daemon patch::parser::tests::parses_control_lines_with_horizontal_whitespace -- --exact --nocapture`
Expected: PASS, proving the parser now accepts padded structural lines without changing payload-line semantics.

- [ ] **Step 5: Add an end-to-end patch RPC regression for tolerant control lines**

```rust
// crates/remote-exec-daemon/tests/patch_rpc.rs
#[tokio::test]
async fn update_move_accepts_horizontal_whitespace_on_control_lines() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("old.txt");
    let destination = fixture.workdir.join("new.txt");
    tokio::fs::write(&source, "old\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    " \t*** Begin Patch\t\n",
                    "\t*** Update File: old.txt  \n",
                    "  *** Move to: new.txt\t\n",
                    " \t@@\t\n",
                    "-old\n",
                    "+new\n",
                    "\t*** End of File \n",
                    "  *** End Patch\t\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M new.txt"));
    assert_eq!(tokio::fs::read_to_string(destination).await.unwrap(), "new\n");
    assert!(tokio::fs::metadata(source).await.is_err());
}
```

- [ ] **Step 6: Run the focused daemon RPC verification**

Run: `cargo test -p remote-exec-daemon --test patch_rpc update_move_accepts_horizontal_whitespace_on_control_lines -- --exact --nocapture`
Expected: PASS, proving the public patch RPC accepts tolerant structural formatting for update, move, and EOF markers.

- [ ] **Step 7: Commit**

```bash
git add crates/remote-exec-daemon/src/patch/parser.rs \
        crates/remote-exec-daemon/tests/patch_rpc.rs
git commit -m "feat: tolerate whitespace on apply_patch control lines"
```

### Task 2: Add Horizontal-Whitespace Tolerance To Exec Interception Wrappers

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Test/Verify: `cargo test -p remote-exec-broker tools::exec_intercept::tests::parses_apply_patch_invocations_with_horizontal_whitespace -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: the broker interception parser has a clear unit seam, and broker integration tests can then prove the tolerated forms still route through patch RPC instead of `/v1/exec/start`.

- [ ] **Step 1: Add a failing interception-parser unit test for whitespace-tolerant wrapper forms**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
#[cfg(test)]
mod tests {
    use super::{InterceptedApplyPatch, maybe_intercept_apply_patch};

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
```

- [ ] **Step 2: Run the focused interception-parser verification and confirm it fails first**

Run: `cargo test -p remote-exec-broker tools::exec_intercept::tests::parses_apply_patch_invocations_with_horizontal_whitespace -- --exact --nocapture`
Expected: FAIL because `split_cd_wrapper(...)` still requires `cd ` with a literal space and the heredoc matcher is still exact-string oriented.

- [ ] **Step 3: Implement token-aware horizontal-whitespace parsing without broadening the grammar**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
fn is_horizontal_whitespace(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

fn trim_horizontal_start(text: &str) -> &str {
    text.trim_start_matches(is_horizontal_whitespace)
}

fn split_cd_wrapper<'a>(cmd: &'a str, workdir: Option<&str>) -> (Option<String>, &'a str) {
    let Some(rest) = cmd.strip_prefix("cd") else {
        return (workdir.map(ToString::to_string), cmd);
    };
    let Some(first) = rest.chars().next() else {
        return (workdir.map(ToString::to_string), cmd);
    };
    if !is_horizontal_whitespace(first) {
        return (workdir.map(ToString::to_string), cmd);
    }

    let rest = trim_horizontal_start(rest);
    let Some((path, tail)) = rest.split_once("&&") else {
        return (workdir.map(ToString::to_string), cmd);
    };
    let path = path.trim_matches(is_horizontal_whitespace);
    if path.is_empty() || path.chars().any(char::is_whitespace) {
        return (workdir.map(ToString::to_string), cmd);
    }

    let mut resolved = workdir.map(PathBuf::from).unwrap_or_default();
    resolved.push(path);
    (
        Some(resolved.display().to_string()),
        trim_horizontal_start(tail),
    )
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
```

- [ ] **Step 4: Run the focused interception-parser verification again**

Run: `cargo test -p remote-exec-broker tools::exec_intercept::tests::parses_apply_patch_invocations_with_horizontal_whitespace -- --exact --nocapture`
Expected: PASS, proving the narrow parser now accepts extra spaces or tabs around `cd`, `&&`, the command name boundary, and `<<`.

- [ ] **Step 5: Add a broker integration regression covering tolerant direct and heredoc forms**

```rust
// crates/remote-exec-broker/tests/mcp_exec.rs
#[tokio::test]
async fn exec_command_intercepts_apply_patch_whitespace_tolerant_forms() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let direct_patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: direct.txt\n",
        "+direct\n",
        "*** End Patch\n",
    );

    let direct = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!(" \tapply_patch\t  '{direct_patch}' \t"),
            }),
        )
        .await;

    assert!(direct.text_output.contains("Process exited with code 0"));
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        direct_patch.to_string()
    );

    let heredoc_fixture = support::spawn_broker_with_stub_daemon().await;
    let heredoc_cmd = concat!(
        "cd\t nested  && \tapplypatch\t <<'PATCH'\n",
        "*** Begin Patch\n",
        "*** Add File: heredoc.txt\n",
        "+heredoc\n",
        "*** End Patch\n",
        "PATCH\n",
    );

    let heredoc = heredoc_fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": heredoc_cmd,
                "workdir": "outer"
            }),
        )
        .await;

    assert!(
        heredoc
            .text_output
            .contains("Output:\nSuccess. Updated the following files:")
    );
    assert_eq!(heredoc_fixture.exec_start_calls().await, 0);
    let forwarded = heredoc_fixture.last_patch_request().await.unwrap();
    assert_eq!(forwarded.workdir, Some("outer/nested".to_string()));
    assert_eq!(
        forwarded.patch,
        concat!(
            "*** Begin Patch\n",
            "*** Add File: heredoc.txt\n",
            "+heredoc\n",
            "*** End Patch\n",
        )
        .to_string()
    );
}
```

- [ ] **Step 6: Run the focused broker verification and the full quality gate**

Run: `cargo test -p remote-exec-broker --test mcp_exec exec_command_intercepts_apply_patch_whitespace_tolerant_forms -- --exact --nocapture`
Expected: PASS, proving tolerant direct and heredoc wrapper forms still intercept and skip `/v1/exec/start`.

Run: `cargo test -p remote-exec-daemon --test patch_rpc`
Expected: PASS, including the new tolerant control-line regression.

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: PASS, including the new tolerant interception regression.

Run: `cargo test --workspace`
Expected: PASS across the workspace.

Run: `cargo fmt --all --check`
Expected: PASS with no formatting diff.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS with no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/remote-exec-broker/src/tools/exec_intercept.rs \
        crates/remote-exec-broker/tests/mcp_exec.rs
git commit -m "feat: tolerate whitespace in apply_patch exec interception"
```
