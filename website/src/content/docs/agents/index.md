---
title: Agent support overview
description: What the sidebar shows for Claude Code, Codex, OpenCode, and Cursor, side by side.
---

Claude Code, Codex, OpenCode, and Cursor (`cursor-agent`) work with the sidebar, but they expose different sets of hooks — so the sidebar's surface area is narrower for Codex, OpenCode, and Cursor than it is for Claude Code.

## Feature support by agent

| Feature                                  | Claude Code | Codex        | OpenCode     | Cursor       | Notes                                                                                                                           |
| ---------------------------------------- | ----------- | ------------ | ------------ | ------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| Base status tracking                    | ✓           | ✓            | ✓            | ✓            | Covers `running`, `idle`, and `error`; `waiting` and `background` depend on agent-specific hooks                                |
| Prompt text display                      | ✓           | ✓            | ✓            | ✓            | Saved from `UserPromptSubmit`                                                                                                   |
| Response text display (`▷ ...`)          | ✓           | ✓            | ✓            | ✓            | Populated from the `Stop` payload; Cursor takes it from `afterAgentResponse`                                                                                                |
| Background shell state                   | ✓           | —            | —            | —            | Claude Bash tools can report `run_in_background`; Codex, OpenCode, and Cursor do not currently document a background Bash flag             |
| Waiting status + wait reason             | ✓           | —            | ✓            | —            | OpenCode maps permission prompts to waiting notifications; Claude also has `Notification`, `PermissionDenied`, and `TeammateIdle` |
| API failure reason display               | ✓           | —            | ✓            | ✓ (generic)  | `StopFailure` is wired for Claude and OpenCode; Cursor's `stop` only reports `status: error` with no detail                                                                             |
| Permission badge                         | ✓ (`plan` / `edit` / `auto` / `!`) | ✓ (`auto` / `!` only) | — | ✓ (`plan` / `auto` / `!`) | Codex and Cursor badges are inferred from process arguments (`--force`, `--auto-review`, `--plan`); OpenCode does not expose permission modes                                     |
| Git branch display                       | ✓           | ✓            | ✓            | ✓            | Uses the pane `cwd`; Claude updates dynamically via `CwdChanged`                                                                |
| Elapsed time                             | ✓           | ✓            | ✓            | ✓            | Since the last prompt                                                                                                            |
| Task progress                            | ✓           | —            | —            | —            | Requires Claude's task tools; Codex fires `PostToolUse` only for `Bash`, and OpenCode / Cursor do not surface task progress                  |
| Task lifecycle notifications             | ✓           | ✓ (`Stop` only) | ✓            | ✓ (`Stop`, `StopFailure`) | `Stop` desktop notifications fire for all three. `Notification`, `TaskCompleted`, `StopFailure`, and `PermissionDenied` vary.   |
| Sub-agent display                        | ✓           | —            | —            | —            | Requires `SubagentStart` / `SubagentStop`; Cursor's `subagentStart` is a permission-gating hook, so it is not wired                                                                                        |
| Activity log                             | ✓           | ✓ (Bash only) | ✓            | ✓            | Codex's `PostToolUse` fires only for `Bash`; OpenCode records the tool events the plugin bridge receives; Cursor records every `postToolUse`                         |
| Worktree lifecycle tracking              | ✓           | —            | —            | —            | Requires `WorktreeCreate` / `WorktreeRemove`                                                                                     |
