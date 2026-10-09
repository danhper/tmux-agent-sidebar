---
title: Codex setup
description: Wire up the Codex hooks from inside a Codex pane.
---

Codex registers hooks through a one-paste flow driven by the sidebar itself.

Before Codex will load hooks, enable the feature flag in `~/.codex/config.toml`:

```toml
[features]
codex_hooks = true
```

If you already have a `[features]` table, add `codex_hooks = true` under it.

## Steps

1. Open a Codex pane in tmux and focus it.
2. Press `prefix + e` to toggle the sidebar. A yellow `ⓘ` badge appears in the top row when required hooks are missing.
3. Click `ⓘ`, then click `[copy]` next to `codex` in the Notices popup.
4. Switch back to the Codex pane and paste. Codex runs `tmux-agent-sidebar setup codex` and merges the hooks into `~/.codex/hooks.json`.
5. Restart Codex after changing `config.toml` so the feature flag takes effect.

## Shared daemon mode (Linux and macOS)

Enable pane-to-thread mapping once:

```bash
tmux-agent-sidebar setup codex-daemon
```

This adds `thread-id` as the final item in `[tui].terminal_title` in
`$CODEX_HOME/config.toml` (default `~/.codex/config.toml`), preserving your other
title items and settings. Restart existing **Codex clients** to apply it; the
daemon can keep running.

The sidebar connects to the existing local daemon's control socket and reads
loaded thread metadata every two seconds. It matches each pane's client-emitted
thread ID to a unique full daemon thread ID. Clients in the same checkout stay
separate, and resuming or switching threads updates the binding. It does not
start a daemon or resume threads.

Codex currently abbreviates the ID in terminal titles. The sidebar accepts an
abbreviation only when it uniquely identifies a loaded thread; ambiguous or
missing IDs are never matched by directory or recency.

Hooks remain useful for prompt/response previews and tool activity. Their events
are routed using the registered thread ID and current client title, rather than
the daemon's inherited `TMUX_PANE`. A thread displayed in multiple panes receives
updates in each attached pane. After a client exits, normal process checks remove
its row even if the daemon keeps the thread loaded.

This requires a Codex version with `tui.terminal_title`'s `thread-id` item and the
local app-server control socket (verified with Codex CLI 0.161.0). Embedded
`codex --no-daemon` clients continue to use their existing hook integration.
