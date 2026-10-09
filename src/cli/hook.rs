use crate::adapter::cursor::is_cursor_payload;
use crate::event::{AgentEvent, resolve_adapter};
use crate::tmux::{CLAUDE_AGENT, CODEX_AGENT, CURSOR_AGENT};

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
    if pane.is_empty() && agent_name != CODEX_AGENT {
        return 0;
    }

    let input = read_stdin_json();
    if is_replayed_cursor_payload(agent_name, &input) {
        return 0;
    }
    let Some(event) = adapter.parse(event_name, &input) else {
        return 0;
    };
    let environment_kind = std::env::var(CLAUDE_ENVIRONMENT_KIND_ENV).ok();
    if is_remote_control_event(agent_name, &event, environment_kind.as_deref()) {
        return 0;
    }

    if agent_name == CODEX_AGENT {
        for (index, pane) in codex_hook_panes(&input, &pane).into_iter().enumerate() {
            handle_event(&pane, agent_name, codex_event_for_pane(&event, index));
        }
        0
    } else {
        handle_event(&pane, agent_name, event)
    }
}

fn codex_event_for_pane(event: &AgentEvent, index: usize) -> AgentEvent {
    let mut event = event.clone();
    // Multiple panes can display a thread, but hook stdout must contain only
    // one protocol response, rather than one JSON object per attached pane.
    if index > 0
        && let AgentEvent::Stop { response, .. } = &mut event
    {
        *response = None;
    }
    event
}

/// Daemon hooks inherit the daemon's environment, not the attached TUI's.
/// Route by the full registered thread ID AND the client's current title.
/// A late event from a previously selected thread cannot replace a new binding.
fn codex_hook_panes(input: &serde_json::Value, inherited_pane: &str) -> Vec<String> {
    let Some(session_id) = input
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };
    let output = crate::tmux::run_tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id}\t#{@pane_session_id}\t#{pane_title}\t#{pane_pid}",
    ])
    .unwrap_or_default();
    let processes = crate::process::ProcessSnapshot::scan();
    let panes = route_codex_hook_panes(
        &output,
        processes.as_ref(),
        session_id,
        inherited_pane,
        std::process::id(),
    );
    if panes.iter().any(|pane| pane == inherited_pane)
        && output.lines().any(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            fields.len() == 4
                && fields[0] == inherited_pane
                && fields[3].parse().ok().is_some_and(|pid| {
                    processes
                        .as_ref()
                        .is_some_and(|p| p.descendants(&[pid]).contains(&std::process::id()))
                })
        })
    {
        crate::tmux::unset_pane_option(inherited_pane, crate::tmux::PANE_CODEX_DAEMON);
    }
    panes
}

fn route_codex_hook_panes(
    output: &str,
    processes: Option<&crate::process::ProcessSnapshot>,
    session_id: &str,
    inherited_pane: &str,
    hook_pid: u32,
) -> Vec<String> {
    let mut panes = Vec::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 4 {
            continue;
        }
        let Some(pid) = fields[3].parse().ok() else {
            continue;
        };
        let Some(processes) = processes else {
            continue;
        };
        if !processes.tree_has_agent(&[pid], &crate::tmux::AgentType::Codex) {
            continue;
        }
        if processes
            .command_lines_for_tree(&[pid])
            .iter()
            .any(|args| args.split_whitespace().any(|arg| arg == "--no-daemon"))
            && !processes.descendants(&[pid]).contains(&hook_pid)
        {
            continue;
        }
        if hook_binding_matches(fields[1], fields[2], session_id) {
            panes.push(fields[0].to_owned());
        }
    }
    if panes.is_empty() && !inherited_pane.is_empty() {
        // Embedded/no-daemon hooks run under the client's pane process tree.
        // A detached shared daemon never passes this ancestry check.
        let genuine_client_hook = output.lines().any(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            fields.len() == 4
                && fields[0] == inherited_pane
                && crate::codex_daemon::title_thread_prefix(fields[2])
                    .is_none_or(|prefix| session_id.starts_with(prefix))
                && fields[3].parse().ok().is_some_and(|pid| {
                    processes.is_some_and(|p| p.descendants(&[pid]).contains(&hook_pid))
                })
        });
        if genuine_client_hook {
            panes.push(inherited_pane.to_owned());
        }
    }
    panes
}

fn hook_binding_matches(registered_id: &str, title: &str, hook_id: &str) -> bool {
    registered_id == hook_id
        && crate::codex_daemon::title_thread_prefix(title)
            .is_some_and(|prefix| hook_id.starts_with(prefix))
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

/// Env var Claude Code sets on sessions spawned by `claude remote-control`
/// (`claude rc`); hooks inherit it from the spawned session.
const CLAUDE_ENVIRONMENT_KIND_ENV: &str = "CLAUDE_CODE_ENVIRONMENT_KIND";

/// Sessions launched through `claude rc` are controlled remotely, so their
/// launcher pane cannot be used as an interactive session. Ignore all bridge
/// hooks, including prompts and stops, so active remote work stays hidden.
fn is_remote_control_event(
    agent_name: &str,
    _event: &AgentEvent,
    environment_kind: Option<&str>,
) -> bool {
    agent_name == CLAUDE_AGENT && environment_kind == Some("bridge")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_stop_fanout_emits_only_one_hook_response() {
        let adapter = resolve_adapter("codex").unwrap();
        let stop = adapter
            .parse("stop", &json!({"session_id":"thread"}))
            .unwrap();
        for index in 0..3 {
            let AgentEvent::Stop { response, .. } = codex_event_for_pane(&stop, index) else {
                panic!("expected Stop");
            };
            assert_eq!(response.is_some(), index == 0);
        }
    }

    #[test]
    fn daemon_hooks_follow_thread_bindings_instead_of_inherited_pane() {
        let first = "019a1234-5678-7123-8123-123456789abc";
        let second = "019a9999-5678-7123-8123-123456789abc";
        let processes = crate::process::ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 codex codex\n200 1 fish fish\n201 200 codex codex\n300 1 fish fish\n400 1 codex codex app-server\n401 400 hook hook\n",
        );
        let output = format!(
            "%1\t{first}\trepo | {}...\t100\n%2\t{second}\trepo | {}...\t200\n%3\t{first}\trepo | {}...\t300\n",
            &first[..29],
            &second[..29],
            &first[..29]
        );
        assert_eq!(
            route_codex_hook_panes(&output, Some(&processes), second, "%1", 401),
            vec!["%2"]
        );
        // A shell retaining an old Codex title must not receive the hook.
        assert_eq!(
            route_codex_hook_panes(&output, Some(&processes), first, "%2", 401),
            vec!["%1"]
        );
        let switched = output.replace(
            &format!("repo | {}...", &first[..29]),
            &format!("repo | {}...", &second[..29]),
        );
        assert_eq!(
            route_codex_hook_panes(&switched, Some(&processes), first, "%1", 401),
            Vec::<String>::new()
        );
        assert!(route_codex_hook_panes(&output, None, first, "%1", 401).is_empty());
    }

    #[test]
    fn embedded_hooks_require_actual_pane_ancestry_for_fallback() {
        let processes = crate::process::ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 codex codex --no-daemon\n102 101 hook hook\n",
        );
        let output = "%1\t\tCodex\t100";
        assert_eq!(
            route_codex_hook_panes(output, Some(&processes), "new-thread", "%1", 102),
            vec!["%1"]
        );
        assert!(
            route_codex_hook_panes(output, Some(&processes), "new-thread", "%1", 999).is_empty()
        );
        let id = "019a1234-5678-7123-8123-123456789abc";
        let mapped = format!("%1\t{id}\trepo | {id}\t100");
        assert!(route_codex_hook_panes(&mapped, Some(&processes), id, "%1", 999).is_empty());
        assert_eq!(
            route_codex_hook_panes(&mapped, Some(&processes), id, "%1", 102),
            vec!["%1"]
        );
    }

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

    fn claude_event(event_name: &str) -> AgentEvent {
        let payload = json!({
            "session_id": "cse_123",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "prompt": "hi",
        });
        resolve_adapter("claude")
            .unwrap()
            .parse(event_name, &payload)
            .expect("claude event parses")
    }

    #[test]
    fn remote_control_session_start_is_dropped() {
        let start = claude_event("session-start");
        assert!(is_remote_control_event("claude", &start, Some("bridge")));
    }

    #[test]
    fn remote_control_prompt_and_stop_are_dropped() {
        let prompt = claude_event("user-prompt-submit");
        assert!(is_remote_control_event("claude", &prompt, Some("bridge")));
        let stop = claude_event("stop");
        assert!(is_remote_control_event("claude", &stop, Some("bridge")));
    }

    #[test]
    fn local_claude_session_start_is_kept() {
        let start = claude_event("session-start");
        assert!(!is_remote_control_event("claude", &start, None));
        assert!(!is_remote_control_event("claude", &start, Some("local")));
        assert!(!is_remote_control_event("codex", &start, Some("bridge")));
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
