---
title: Cursor setup
description: Wire up the Cursor CLI (cursor-agent) hooks from inside a Cursor pane.
---

Cursor's CLI registers hooks through the same one-paste flow as Codex. The
hooks land in `~/.cursor/hooks.json`, which the Cursor IDE reads too — that is
harmless, because the sidebar ignores hook invocations that do not come from a
tmux pane.

:::note
Cursor's Claude Code compatibility layer also executes the hooks it finds in
`.claude/settings.json`, but only a subset of them (in practice `SessionStart`,
`PreToolUse`, `PostToolUse`, `SessionEnd`) and with Cursor-native payloads.
The sidebar recognises and ignores those replays, so an existing Claude Code
setup does **not** make Cursor panes appear — `~/.cursor/hooks.json` is the
only supported wiring.
:::

## Steps

1. Open a `cursor-agent` pane in tmux and focus it.
2. Press `prefix + e` to toggle the sidebar. A yellow `ⓘ` badge appears in the top row when required hooks are missing.
3. Click `ⓘ`, then click `[copy]` next to `cursor` in the Notices popup.
4. Switch back to the Cursor pane and paste. Cursor runs `tmux-agent-sidebar setup cursor` and merges the hooks into `~/.cursor/hooks.json`.
5. Restart `cursor-agent` so it picks up the new hooks.

## Manual setup

If you prefer to edit the file yourself, print the snippet and merge it:

```sh
~/.tmux/plugins/tmux-agent-sidebar/target/release/tmux-agent-sidebar setup cursor
```

The output is a complete Cursor `hooks.json` document (`"version": 1` plus a
`hooks` object keyed by Cursor event name). A project-level
`.cursor/hooks.json` works as well if you only want the sidebar for one
repository.
