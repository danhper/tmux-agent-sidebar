use serde_json::Value;

use crate::event::{AgentEvent, AgentEventKind, EventAdapter};
use crate::tmux::CURSOR_AGENT;
use crate::tool_name::CanonicalTool;

use super::{HookRegistration, json_str, json_value_or_null, optional_str};

pub struct CursorAdapter;

/// Whether a hook payload was produced by Cursor, regardless of which agent
/// name the hook was invoked under. Cursor stamps `cursor_version` on every
/// hook payload (including the Claude-format triggers its Claude Code
/// compatibility layer replays from `.claude/settings.json`), and no other
/// supported agent emits that key.
pub fn is_cursor_payload(input: &Value) -> bool {
    input.get("cursor_version").is_some()
}

impl CursorAdapter {
    /// Single source of truth for Cursor CLI (`cursor-agent`) hook wiring.
    /// Verified against the Cursor hooks reference (cursor.com/docs/agent/hooks)
    /// and against live payloads captured from `cursor-agent` running inside
    /// tmux. Hooks live in `~/.cursor/hooks.json` (`{"version": 1, "hooks":
    /// {...}}`) and are shared with the Cursor IDE; the IDE never sets
    /// `$TMUX_PANE`, so its invocations exit early in `cmd_hook`.
    ///
    /// Only *observation* hooks are registered. Cursor's gating hooks
    /// (`preToolUse`, `beforeShellExecution`, `beforeReadFile`,
    /// `beforeMCPExecution`, `subagentStart`) expect a `permission` verdict
    /// on stdout, and a hook that answers without one is treated as a deny —
    /// so they are deliberately left out.
    ///
    /// Caveats:
    /// - `stop` carries only `status` (`completed` / `aborted` / `error`)
    ///   and no assistant text. `afterAgentResponse` fires immediately after
    ///   `stop` with the final `text`, so both triggers map to `Stop`: the
    ///   first flips the pane to idle, the second fills the `▷` response
    ///   preview. `status: "error"` is surfaced as `StopFailure` instead.
    /// - No `cwd` field is present at the top level; the workspace root is
    ///   read from `workspace_roots[0]`.
    /// - Cursor has no Claude-style `Notification` hook, so permission
    ///   prompts cannot be surfaced as `waiting`.
    pub const HOOK_REGISTRATIONS: &'static [HookRegistration] = &[
        HookRegistration {
            trigger: "sessionStart",
            matcher: None,
            kind: AgentEventKind::SessionStart,
        },
        HookRegistration {
            trigger: "sessionEnd",
            matcher: None,
            kind: AgentEventKind::SessionEnd,
        },
        HookRegistration {
            trigger: "beforeSubmitPrompt",
            matcher: None,
            kind: AgentEventKind::UserPromptSubmit,
        },
        HookRegistration {
            trigger: "stop",
            matcher: None,
            kind: AgentEventKind::Stop,
        },
        HookRegistration {
            trigger: "afterAgentResponse",
            matcher: None,
            kind: AgentEventKind::Stop,
        },
        HookRegistration {
            trigger: "postToolUse",
            matcher: None,
            kind: AgentEventKind::ActivityLog,
        },
    ];
}

/// Cursor payloads carry the project directory in `workspace_roots` (an
/// array — multi-root workspaces list several) rather than a top-level
/// `cwd`. Prefer the first root and fall back to `cwd` for forward
/// compatibility.
fn workspace_cwd(input: &Value) -> String {
    input
        .get("workspace_roots")
        .and_then(Value::as_array)
        .and_then(|roots| roots.first())
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| json_str(input, "cwd").to_string())
}

/// `session_id` is present on every hook; `conversation_id` carries the
/// same value and is the documented common field, so accept it as a
/// fallback.
fn session_id(input: &Value) -> Option<String> {
    optional_str(input, "session_id").or_else(|| optional_str(input, "conversation_id"))
}

/// Cursor tool names are PascalCase like Claude's but a few differ
/// (`Shell` for the terminal, `Task` for sub-agents, `StrReplace` for
/// edits). Normalise the known ones into the shared [`CanonicalTool`]
/// vocabulary so the activity-log label extractor and colour table apply;
/// everything else (`Delete`, `MCP:<server>`, …) passes through verbatim.
fn normalize_tool_name(raw: &str) -> String {
    let canonical = match raw {
        "Shell" => CanonicalTool::Bash,
        "Task" => CanonicalTool::Agent,
        "StrReplace" | "SearchReplace" | "MultiEdit" | "Edit" => CanonicalTool::Edit,
        other => return other.to_string(),
    };
    canonical.as_str().to_string()
}

/// `postToolUse` delivers `tool_output` as a JSON-encoded *string*
/// (e.g. `"{\"output\":\"\",\"exitCode\":0}"`). Decode it so downstream
/// consumers see a structured value; keep the raw string when it is not
/// valid JSON, and accept an already-structured `tool_response` too.
fn parse_tool_output(input: &Value) -> Value {
    match input.get("tool_output") {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or(Value::String(raw.clone())),
        Some(Value::Null) | None => json_value_or_null(input, "tool_response"),
        Some(other) => other.clone(),
    }
}

impl EventAdapter for CursorAdapter {
    fn parse(&self, event_name: &str, input: &Value) -> Option<AgentEvent> {
        match event_name {
            "session-start" => Some(AgentEvent::SessionStart {
                agent: CURSOR_AGENT.into(),
                cwd: workspace_cwd(input),
                permission_mode: String::new(),
                // Cursor does not distinguish fresh starts from `--resume`
                // in the payload, so no resume marker is written.
                source: String::new(),
                worktree: None,
                agent_id: None,
                session_id: session_id(input),
            }),
            "session-end" => Some(AgentEvent::SessionEnd {
                end_reason: json_str(input, "reason").into(),
            }),
            "user-prompt-submit" => Some(AgentEvent::UserPromptSubmit {
                agent: CURSOR_AGENT.into(),
                cwd: workspace_cwd(input),
                permission_mode: String::new(),
                prompt: json_str(input, "prompt").into(),
                worktree: None,
                agent_id: None,
                session_id: session_id(input),
            }),
            // Shared by the `stop` trigger (carries `status`, no text) and
            // `afterAgentResponse` (carries the final `text`, no status).
            "stop" => {
                if json_str(input, "status") == "error" {
                    return Some(AgentEvent::StopFailure {
                        agent: CURSOR_AGENT.into(),
                        cwd: workspace_cwd(input),
                        permission_mode: String::new(),
                        error: "error".into(),
                        worktree: None,
                        agent_id: None,
                        session_id: session_id(input),
                    });
                }
                Some(AgentEvent::Stop {
                    agent: CURSOR_AGENT.into(),
                    cwd: workspace_cwd(input),
                    permission_mode: String::new(),
                    last_message: json_str(input, "text").into(),
                    response: None,
                    worktree: None,
                    agent_id: None,
                    session_id: session_id(input),
                })
            }
            "activity-log" => {
                let raw_name = json_str(input, "tool_name");
                if raw_name.is_empty() {
                    return None;
                }
                Some(AgentEvent::ActivityLog {
                    tool_name: normalize_tool_name(raw_name),
                    tool_input: json_value_or_null(input, "tool_input"),
                    tool_response: parse_tool_output(input),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Realistic common envelope captured from `cursor-agent 2026.08.25`.
    fn envelope(extra: Value) -> Value {
        let mut base = json!({
            "conversation_id": "6c1e6af8-86dd-4412-ab55-89877f9ec5ab",
            "generation_id": "83a9817c-d72c-4d0d-a4a5-1f8a90033fdb",
            "model": "auto-smart",
            "session_id": "6c1e6af8-86dd-4412-ab55-89877f9ec5ab",
            "cursor_version": "2026.08.25-3e8eec8",
            "workspace_roots": ["/home/user/project"],
            "user_email": null,
            "transcript_path": null,
        });
        if let (Value::Object(base), Value::Object(extra)) = (&mut base, extra) {
            base.extend(extra);
        }
        base
    }

    #[test]
    fn is_cursor_payload_keys_off_cursor_version() {
        assert!(is_cursor_payload(&envelope(json!({}))));
        assert!(is_cursor_payload(
            &json!({"cursor_version": "2026.08.25-3e8eec8", "hook_event_name": "sessionStart"})
        ));
        // Claude Code / Codex / OpenCode payloads never carry the key.
        assert!(!is_cursor_payload(
            &json!({"session_id": "abc", "cwd": "/tmp"})
        ));
        assert!(!is_cursor_payload(
            &json!({"hook_event_name": "SessionStart"})
        ));
        assert!(!is_cursor_payload(&Value::Null));
    }

    #[test]
    fn hook_registrations_match_parse_arms() {
        super::super::assert_table_drift_free("cursor", CursorAdapter::HOOK_REGISTRATIONS);
    }

    #[test]
    fn session_start_reads_workspace_root_as_cwd() {
        let input = envelope(json!({
            "hook_event_name": "sessionStart",
            "is_background_agent": false,
        }));
        let event = CursorAdapter.parse("session-start", &input).unwrap();
        assert_eq!(
            event,
            AgentEvent::SessionStart {
                agent: CURSOR_AGENT.into(),
                cwd: "/home/user/project".into(),
                permission_mode: "".into(),
                source: "".into(),
                worktree: None,
                agent_id: None,
                session_id: Some("6c1e6af8-86dd-4412-ab55-89877f9ec5ab".into()),
            }
        );
    }

    #[test]
    fn session_start_falls_back_to_cwd_without_workspace_roots() {
        let event = CursorAdapter
            .parse("session-start", &json!({"cwd": "/tmp/x"}))
            .unwrap();
        match event {
            AgentEvent::SessionStart { cwd, .. } => assert_eq!(cwd, "/tmp/x"),
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_start_missing_fields_default_to_empty() {
        let event = CursorAdapter.parse("session-start", &json!({})).unwrap();
        assert_eq!(
            event,
            AgentEvent::SessionStart {
                agent: "cursor".into(),
                cwd: "".into(),
                permission_mode: "".into(),
                source: "".into(),
                worktree: None,
                agent_id: None,
                session_id: None,
            }
        );
    }

    #[test]
    fn session_id_falls_back_to_conversation_id() {
        let event = CursorAdapter
            .parse("session-start", &json!({"conversation_id": "conv-1"}))
            .unwrap();
        match event {
            AgentEvent::SessionStart { session_id, .. } => {
                assert_eq!(session_id.as_deref(), Some("conv-1"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_end_captures_reason() {
        let input = envelope(json!({
            "hook_event_name": "sessionEnd",
            "reason": "completed",
            "duration_ms": 227088,
            "final_status": "completed",
        }));
        assert_eq!(
            CursorAdapter.parse("session-end", &input),
            Some(AgentEvent::SessionEnd {
                end_reason: "completed".into()
            })
        );
    }

    #[test]
    fn user_prompt_submit_from_before_submit_prompt() {
        let input = envelope(json!({
            "hook_event_name": "beforeSubmitPrompt",
            "prompt": "Create a file called note.txt",
            "attachments": [],
        }));
        let event = CursorAdapter.parse("user-prompt-submit", &input).unwrap();
        assert_eq!(
            event,
            AgentEvent::UserPromptSubmit {
                agent: CURSOR_AGENT.into(),
                cwd: "/home/user/project".into(),
                permission_mode: "".into(),
                prompt: "Create a file called note.txt".into(),
                worktree: None,
                agent_id: None,
                session_id: Some("6c1e6af8-86dd-4412-ab55-89877f9ec5ab".into()),
            }
        );
    }

    #[test]
    fn stop_completed_has_no_response_text() {
        let input = envelope(json!({
            "hook_event_name": "stop",
            "status": "completed",
            "loop_count": 0,
        }));
        let event = CursorAdapter.parse("stop", &input).unwrap();
        assert_eq!(
            event,
            AgentEvent::Stop {
                agent: CURSOR_AGENT.into(),
                cwd: "/home/user/project".into(),
                permission_mode: "".into(),
                last_message: "".into(),
                response: None,
                worktree: None,
                agent_id: None,
                session_id: Some("6c1e6af8-86dd-4412-ab55-89877f9ec5ab".into()),
            }
        );
    }

    #[test]
    fn stop_aborted_is_a_plain_stop() {
        let event = CursorAdapter
            .parse("stop", &json!({"status": "aborted"}))
            .unwrap();
        assert!(matches!(event, AgentEvent::Stop { .. }));
    }

    #[test]
    fn stop_error_status_maps_to_stop_failure() {
        let input = envelope(json!({"hook_event_name": "stop", "status": "error"}));
        let event = CursorAdapter.parse("stop", &input).unwrap();
        assert_eq!(
            event,
            AgentEvent::StopFailure {
                agent: CURSOR_AGENT.into(),
                cwd: "/home/user/project".into(),
                permission_mode: "".into(),
                error: "error".into(),
                worktree: None,
                agent_id: None,
                session_id: Some("6c1e6af8-86dd-4412-ab55-89877f9ec5ab".into()),
            }
        );
    }

    #[test]
    fn after_agent_response_payload_fills_last_message() {
        // `afterAgentResponse` is wired to the same `stop` event and is the
        // only place the assistant text is available.
        let input = envelope(json!({
            "hook_event_name": "afterAgentResponse",
            "text": "Done: note.txt now contains the single line hello.",
        }));
        let event = CursorAdapter.parse("stop", &input).unwrap();
        match event {
            AgentEvent::Stop {
                last_message,
                response,
                ..
            } => {
                assert_eq!(
                    last_message,
                    "Done: note.txt now contains the single line hello."
                );
                assert!(response.is_none(), "Cursor stop hooks print nothing");
            }
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[test]
    fn activity_log_shell_normalizes_to_bash_and_decodes_output() {
        let input = envelope(json!({
            "hook_event_name": "postToolUse",
            "tool_name": "Shell",
            "tool_input": {"command": "echo probe-ok", "cwd": "", "timeout": 30000},
            "tool_output": "{\"output\":\"probe-ok\\n\",\"exitCode\":0}",
            "duration": 346.784,
            "tool_use_id": "19a4fed9",
            "cwd": "",
        }));
        let event = CursorAdapter.parse("activity-log", &input).unwrap();
        match event {
            AgentEvent::ActivityLog {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input["command"], "echo probe-ok");
                assert_eq!(tool_response["exitCode"], 0);
                assert_eq!(tool_response["output"], "probe-ok\n");
            }
            other => panic!("expected ActivityLog, got {other:?}"),
        }
    }

    #[test]
    fn activity_log_write_keeps_file_path_key() {
        let input = envelope(json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "/home/user/project/note.txt", "content": "hello\n"},
            "tool_output": "{\"file_path\":\"/home/user/project/note.txt\",\"success\":true}",
        }));
        let event = CursorAdapter.parse("activity-log", &input).unwrap();
        match event {
            AgentEvent::ActivityLog {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input["file_path"], "/home/user/project/note.txt");
                assert_eq!(tool_response["success"], true);
            }
            other => panic!("expected ActivityLog, got {other:?}"),
        }
    }

    #[test]
    fn activity_log_task_maps_to_agent_and_str_replace_to_edit() {
        for (raw, expected) in [
            ("Task", "Agent"),
            ("StrReplace", "Edit"),
            ("Read", "Read"),
            ("Grep", "Grep"),
        ] {
            let event = CursorAdapter
                .parse("activity-log", &json!({"tool_name": raw}))
                .unwrap();
            match event {
                AgentEvent::ActivityLog { tool_name, .. } => assert_eq!(tool_name, expected),
                other => panic!("expected ActivityLog, got {other:?}"),
            }
        }
    }

    #[test]
    fn activity_log_mcp_and_unknown_tools_pass_through() {
        for raw in ["MCP:github", "Delete", "ListDir"] {
            let event = CursorAdapter
                .parse("activity-log", &json!({"tool_name": raw}))
                .unwrap();
            match event {
                AgentEvent::ActivityLog { tool_name, .. } => assert_eq!(tool_name, raw),
                other => panic!("expected ActivityLog, got {other:?}"),
            }
        }
    }

    #[test]
    fn activity_log_non_json_tool_output_is_kept_as_string() {
        let event = CursorAdapter
            .parse(
                "activity-log",
                &json!({"tool_name": "Shell", "tool_output": "plain text"}),
            )
            .unwrap();
        match event {
            AgentEvent::ActivityLog { tool_response, .. } => {
                assert_eq!(tool_response, Value::String("plain text".into()));
            }
            other => panic!("expected ActivityLog, got {other:?}"),
        }
    }

    #[test]
    fn activity_log_empty_tool_name_rejected() {
        assert!(CursorAdapter.parse("activity-log", &json!({})).is_none());
    }

    #[test]
    fn claude_only_events_not_supported() {
        for name in [
            "notification",
            "stop-failure",
            "permission-denied",
            "cwd-changed",
            "subagent-start",
            "subagent-stop",
            "task-created",
            "task-completed",
            "teammate-idle",
            "worktree-create",
            "worktree-remove",
            "something-else",
        ] {
            assert!(
                CursorAdapter
                    .parse(name, &json!({"agent_type": "X"}))
                    .is_none(),
                "{name} should be ignored by the Cursor adapter"
            );
        }
    }
}
