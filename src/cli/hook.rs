use crate::adapter::cursor::is_cursor_payload;
use crate::event::{AgentEvent, resolve_adapter};
use crate::tmux::CURSOR_AGENT;

use super::{read_stdin_json, tmux_pane};

mod activity;
mod context;
mod handlers;
mod notifications;

use context::sync_pane_location;
use notifications::notification_settings;

// ─── hook subcommand ────────────────────────────────────────────────────────

pub(crate) fn cmd_hook(args: &[String]) -> i32 {
    let agent_name = args.first().map(|s| s.as_str()).unwrap_or("");
    let event_name = args.get(1).map(|s| s.as_str()).unwrap_or("");

    if agent_name.is_empty() || event_name.is_empty() {
        return 0;
    }

    let Some(adapter) = resolve_adapter(agent_name) else {
        return 0;
    };

    let pane = tmux_pane();
    if pane.is_empty() {
        return 0;
    }

    let input = read_stdin_json();
    if is_replayed_cursor_payload(agent_name, &input) {
        return 0;
    }
    let Some(event) = adapter.parse(event_name, &input) else {
        return 0;
    };

    handle_event(&pane, agent_name, event)
}

// ─── event handler ──────────────────────────────────────────────────────────

fn handle_event(pane: &str, agent_name: &str, event: AgentEvent) -> i32 {
    match event {
        AgentEvent::SessionStart {
            agent,
            cwd,
            permission_mode,
            source,
            worktree,
            session_id,
            ..
        } => handlers::on_session_start(
            pane,
            &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
            &source,
        ),
        AgentEvent::SessionEnd { end_reason } => {
            let notifications = notification_settings();
            handlers::on_session_end(pane, agent_name, &end_reason, &notifications)
        }
        AgentEvent::UserPromptSubmit {
            agent,
            cwd,
            permission_mode,
            prompt,
            worktree,
            session_id,
            ..
        } => handlers::on_user_prompt_submit(
            pane,
            &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
            &prompt,
        ),
        AgentEvent::Notification {
            agent,
            cwd,
            permission_mode,
            wait_reason,
            meta_only,
            worktree,
            session_id,
            ..
        } => {
            let notifications = notification_settings();
            handlers::on_notification(
                pane,
                &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
                &wait_reason,
                meta_only,
                &notifications,
            )
        }
        AgentEvent::Stop {
            agent,
            cwd,
            permission_mode,
            last_message,
            response,
            worktree,
            session_id,
            ..
        } => {
            let notifications = notification_settings();
            handlers::on_stop(
                pane,
                &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
                &last_message,
                response.as_deref(),
                &notifications,
            )
        }
        AgentEvent::StopFailure {
            agent,
            cwd,
            permission_mode,
            error,
            worktree,
            session_id,
            ..
        } => {
            let notifications = notification_settings();
            handlers::on_stop_failure(
                pane,
                &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
                &error,
                &notifications,
            )
        }
        AgentEvent::SubagentStart {
            agent_type,
            agent_id,
        } => handlers::on_subagent_start(pane, &agent_type, agent_id.as_deref()),
        AgentEvent::SubagentStop { agent_id, .. } => {
            handlers::on_subagent_stop(pane, agent_id.as_deref())
        }
        AgentEvent::ActivityLog {
            tool_name,
            tool_input,
            tool_response,
        } => activity::handle_activity_log(pane, &tool_name, &tool_input, &tool_response),
        AgentEvent::PermissionDenied {
            agent,
            cwd,
            permission_mode,
            worktree,
            session_id,
            ..
        } => {
            let notifications = notification_settings();
            handlers::on_permission_denied(
                pane,
                &context::make_ctx(&agent, &cwd, &permission_mode, &worktree, &session_id),
                &notifications,
            )
        }
        AgentEvent::CwdChanged {
            cwd,
            worktree,
            session_id,
            ..
        } => {
            sync_pane_location(pane, &cwd, &worktree, &session_id);
            0
        }
        AgentEvent::TaskCreated { .. } => 0,
        AgentEvent::TaskCompleted {
            task_id,
            task_subject,
        } => {
            super::set_attention(pane, "notification");
            let notifications = notification_settings();
            handlers::on_task_completed(pane, agent_name, &task_id, &task_subject, &notifications)
        }
        AgentEvent::TeammateIdle {
            teammate_name,
            idle_reason,
            ..
        } => handlers::on_teammate_idle(pane, &teammate_name, &idle_reason),
        AgentEvent::WorktreeCreate => 0,
        AgentEvent::WorktreeRemove { .. } => handlers::on_worktree_remove(pane),
    }
}

/// Cursor's Claude Code compatibility layer executes the hooks it finds in
/// `.claude/settings.json` (user or project level) under Claude's trigger
/// names. Cursor documents a mapping for a subset of Claude triggers
/// (`SessionStart`, `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`,
/// `SubagentStop`, `SessionEnd`, `PreCompact`); on `cursor-agent`
/// 2026.08.25 only `SessionStart` / `PreToolUse` / `PostToolUse` /
/// `SessionEnd` were observed to fire, and every payload is Cursor-native
/// (`workspace_roots`, `cursor_version`, no `cwd`). Letting those through
/// would tag a `cursor-agent` pane as `claude`, leave it stuck on `running`
/// after the first tool call when no `Stop` arrives, and, once the user
/// installs the real `~/.cursor/hooks.json` hooks, duplicate every
/// activity-log entry. Cursor panes are tracked exclusively through the
/// `cursor` adapter, so anything not addressed to it is dropped.
fn is_replayed_cursor_payload(agent_name: &str, input: &serde_json::Value) -> bool {
    agent_name != CURSOR_AGENT && is_cursor_payload(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cursor_payload() -> serde_json::Value {
        json!({
            "hook_event_name": "sessionStart",
            "cursor_version": "2026.08.25-3e8eec8",
            "session_id": "6c1e6af8",
            "workspace_roots": ["/home/user/project"],
        })
    }

    #[test]
    fn cursor_payload_under_claude_name_is_dropped() {
        assert!(is_replayed_cursor_payload("claude", &cursor_payload()));
        assert!(is_replayed_cursor_payload("codex", &cursor_payload()));
    }

    #[test]
    fn cursor_payload_under_cursor_name_is_kept() {
        assert!(!is_replayed_cursor_payload("cursor", &cursor_payload()));
    }

    #[test]
    fn native_claude_payload_is_kept() {
        let claude = json!({
            "hook_event_name": "SessionStart",
            "session_id": "abc",
            "cwd": "/home/user/project",
            "permission_mode": "default",
        });
        assert!(!is_replayed_cursor_payload("claude", &claude));
        assert!(!is_replayed_cursor_payload(
            "claude",
            &serde_json::Value::Null
        ));
    }
}
