---
title: Cursor
description: What the sidebar shows for Cursor CLI (cursor-agent) panes, and what is not available due to the Cursor hook schema.
---

Cursor's CLI (`cursor-agent`) fires hooks from `~/.cursor/hooks.json`. Only its
*observation* hooks are wired — the permission-gating hooks (`preToolUse`,
`beforeShellExecution`, `beforeReadFile`, `subagentStart`, …) expect a verdict
on stdout and would block the agent if the sidebar answered them, so the
visible surface is close to Codex.

## What you get

### Status and prompts

- Live status from `sessionStart` / `beforeSubmitPrompt` / `stop`
- Prompt text from `beforeSubmitPrompt`
- Response preview (`▷ …`) from `afterAgentResponse`, which fires right after `stop`
- Elapsed time since the last prompt
- Clean teardown from `sessionEnd`, with the shell-fallback sweep as a safety net for crashes

### Git

- Branch display from the pane's workspace root (`workspace_roots[0]`)
- PR number (needs `gh` CLI)

### Permission badges

Inferred from the `cursor-agent` process arguments:

- `!` — `--force`, `-f`, or `--yolo`
- `auto` — `--auto-review`
- `plan` — `--plan` or `--mode plan`

### Notifications

- `stop` — the assistant finished responding.
- `stop_failure` — the turn ended with `status: error`.

### Activity log

- Every `postToolUse` event. Cursor's `Shell` tool is shown as `Bash`, `Task`
  as `Agent`, and `StrReplace` as `Edit`; MCP tools keep their `MCP:<server>`
  name.

## What is not available

| Feature                      | Why                                                                                                   |
| ---------------------------- | ----------------------------------------------------------------------------------------------------- |
| Waiting status + wait reason | Cursor has no `Notification`-style hook; its permission prompts are only visible through gating hooks |
| Background shell state       | Cursor's `Shell` payload does not include a background flag                                           |
| Error detail                 | `stop` reports only `status: error`; the sidebar shows a generic `error` reason                        |
| Task progress counter        | Cursor has no task-list tools                                                                          |
| Sub-agent tree               | `subagentStart` is a permission-gating hook, so it is not registered                                  |
| Worktree lifecycle tracking  | Cursor does not emit `WorktreeCreate` / `WorktreeRemove`                                               |
| `edit` permission badge      | Cursor has no `acceptEdits` equivalent                                                                 |

## Setup

Wire the hooks from inside a Cursor pane — see [Cursor setup](/tmux-agent-sidebar/getting-started/cursor/).
Hooks that Cursor replays from a Claude Code `.claude/settings.json` are
ignored on purpose; only `~/.cursor/hooks.json` (or a project-level
`.cursor/hooks.json`) drives the sidebar.
