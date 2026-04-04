---
title: using-remote-exec-mcp Skill Design
date: 2026-04-04
status: approved-for-planning
---

# using-remote-exec-mcp Skill Design

## Goal

Add a repo-local skill at `skills/using-remote-exec-mcp/` that teaches agents how to use the public `remote-exec-mcp` tools for remote work:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`

The skill is for agents using the tool surface, not for developers working on this repository. It must avoid repository internals, implementation details, component breakdowns, language choices, and architectural explanation.

## User-Approved Scope

The skill should:

- live at `skills/using-remote-exec-mcp/`
- teach practical usage patterns rather than schema-by-schema API reference
- frame `remote-exec-mcp` as a specialized toolset for remote work, not the default toolset for ordinary local work
- include a specific note for Codex agents: use the four overlapping tools the same way they use the internal local-system tools, with the added remote concepts around targets and file transfer

The skill should not:

- teach repository structure
- explain broker or daemon responsibilities
- describe internal trust model or transport details
- focus on implementation or maintenance of `remote-exec-mcp`

## Recommended Skill Shape

Use a hybrid structure that combines fast tool selection with practical workflows.

This is preferred over:

- a very short quick-start, which would be too thin for real remote workflows
- a workflow-only guide, which would hide important per-tool judgment

## Skill Structure

### 1. Overview

Open with a short explanation that this skill teaches practical remote use of the exposed tools. Make it clear that the skill is about operating on remote targets and moving files across endpoints when needed.

Include a special Codex note:

- for Codex agents, approach `exec_command`, `write_stdin`, `apply_patch`, and `view_image` the same way as the internal local-system tools
- the main added concerns are target selection and explicit file transfer where needed

### 2. When to Use

Describe the trigger conditions in searchable terms. The frontmatter description should focus only on when to load the skill, not on the workflow inside it.

Core triggers:

- work must happen on a named remote target
- files need to be inspected or modified on a remote endpoint
- an interactive remote command session must be continued
- files need to move between local and remote endpoints or between two remote targets
- an image must be inspected on a remote target

Explicit non-trigger:

- purely local work that does not need a remote target

### 3. Tool Selection Guide

Provide a fast decision guide that helps an agent pick the correct tool without reading the whole skill first.

Expected decisions:

- `list_targets` to discover valid target names
- `exec_command` to run commands on one target
- `write_stdin` to continue a live session created by `exec_command`
- `apply_patch` to make direct file edits on one target
- `view_image` to inspect an image file on one target
- `transfer_files` to move content between local and remote endpoints or across remote targets

### 4. Practical Patterns

Give each tool a short practical section focused on habits and decision-making instead of field-by-field schema coverage.

Expected guidance:

- `list_targets`
  - check target names before assuming them
  - use it early in a remote workflow
- `exec_command`
  - always be explicit about target and working directory intent
  - prefer straightforward non-interactive commands unless a live session is actually needed
  - use it for inspection, builds, tests, and command-driven file discovery on a target
- `write_stdin`
  - use only after `exec_command` returns a live `session_id`
  - use it for prompts, REPLs, editors, long-running tasks, or interactive shells
- `apply_patch`
  - prefer direct patching over shell-driven file editing when changing files on one target
  - use the same editing judgment as with Codex internal tooling
- `view_image`
  - inspect the remote image where it already exists instead of transferring it first unless there is a reason to relocate it
- `transfer_files`
  - use it to stage inputs to a remote target, retrieve outputs, or relay content between endpoints
  - treat it as the standard bridge when data must move between machines

### 5. Common Remote Workflows

Include compact end-to-end recipes that show how the tools fit together.

Planned workflows:

- discover targets, inspect a remote tree, patch a file, then run verification remotely
- transfer a local file or directory to a remote target, execute it there, and bring back outputs if needed
- start an interactive remote command and continue it with `write_stdin`
- inspect a remote screenshot or other remote image asset with `view_image`
- move content between two remote targets with `transfer_files`

## Writing Constraints

The skill should be concise and searchable, not exhaustive.

Constraints:

- optimize for future agents deciding what to do next
- prefer actionable rules and examples over exhaustive parameter lists
- avoid repo-specific jargon and internal names beyond the public tool names
- avoid implementation detail that would distract from usage
- keep examples short and operational

## Error Handling Guidance To Teach

The skill should teach operational caution without becoming an error catalog.

Include lightweight guidance such as:

- verify target names first rather than guessing
- expect remote file access and command failures to be target-specific
- use `write_stdin` only with a valid active session
- choose `transfer_files` when data must cross endpoints instead of trying to fake remote-local coupling through shell commands

## Validation Plan

Because this is a new documentation-oriented skill rather than a behavior-enforcement change to an existing skill, lightweight validation is sufficient.

Validation should include:

- confirm the skill directory and `SKILL.md` file are created at the intended repo-local path
- check frontmatter for valid `name` and `description`
- review the wording for searchability and trigger clarity
- ensure the content stays consumer-focused and does not leak repository implementation details
- ensure all six public tools are covered with practical guidance

## Deliverables

- `skills/using-remote-exec-mcp/SKILL.md`

No supporting files are required unless the main skill becomes too long, which is not expected for this change.
