use super::*;
use std::os::unix::net::UnixListener;

const FIRST: &str = "019a1234-5678-7123-8123-123456789abc";
const SECOND: &str = "019a9999-5678-7123-8123-123456789abc";

fn thread(id: &str, state: Value) -> Thread {
    parse_thread(&json!({
        "id": id, "name":"Fix parser", "cwd":"/repo", "preview":"Check parser",
        "status":state
    }))
    .unwrap()
}

#[test]
fn title_binding_distinguishes_clients_in_the_same_checkout() {
    let threads = HashMap::from([
        (FIRST.into(), thread(FIRST, json!({"type":"idle"}))),
        (
            SECOND.into(),
            thread(SECOND, json!({"type":"active","activeFlags":[]})),
        ),
    ]);
    let title = format!("⠇ Fix parser | repo | {}...", &FIRST[..29]);
    assert_eq!(thread_for_title(&title, &threads).unwrap().id, FIRST);
    let switched = format!("Fix parser | repo | {SECOND}");
    assert_eq!(thread_for_title(&switched, &threads).unwrap().id, SECOND);
    assert!(thread_for_title("Fix parser | repo", &threads).is_none());
    assert!(thread_for_title("019a1234-5678-7123-8123-123456789abc | repo", &threads).is_none());
}

#[test]
fn ambiguous_prefixes_and_malformed_ids_are_never_bound() {
    let collision = "019a1234-5678-7123-8123-123456789def";
    let threads = HashMap::from([
        (FIRST.into(), thread(FIRST, json!({"type":"idle"}))),
        (collision.into(), thread(collision, json!({"type":"idle"}))),
    ]);
    assert!(thread_for_title(&format!("{}...", &FIRST[..29]), &threads).is_none());
    for title in [
        "",
        "thread-id",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaa...",
        "019a1234-5678-7123...",
        "019a1234-5678-7123-8123-123456789abz",
    ] {
        assert!(title_thread_prefix(title).is_none(), "{title}");
    }
}

#[test]
fn daemon_status_covers_active_idle_approvals_input_and_errors() {
    for (state, status, reason) in [
        (
            json!({"type":"active","activeFlags":[]}),
            PaneStatus::Running,
            "",
        ),
        (
            json!({"type":"active","activeFlags":["waitingOnApproval"]}),
            PaneStatus::Waiting,
            "permission",
        ),
        (
            json!({"type":"active","activeFlags":["waitingOnUserInput"]}),
            PaneStatus::Waiting,
            "input",
        ),
        (json!({"type":"idle"}), PaneStatus::Idle, ""),
        (json!({"type":"systemError"}), PaneStatus::Error, ""),
        (json!({"type":"notLoaded"}), PaneStatus::Unknown, ""),
    ] {
        let thread = thread(FIRST, state);
        assert_eq!(thread.status, status);
        assert_eq!(thread.wait_reason, reason);
    }
}

#[test]
fn setup_preserves_existing_config_and_is_idempotent() {
    let config = "# Keep this comment\nmodel = \"custom-model\"\n[tui]\nanimations = false\nterminal_title = [\"project-name\", \"session-id\", \"status\"]\n";
    let updated = configure_title(config).unwrap();
    let doc = updated.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["model"].as_str(), Some("custom-model"));
    assert_eq!(doc["tui"]["animations"].as_bool(), Some(false));
    assert!(updated.starts_with("# Keep this comment\n"));
    assert_eq!(
        doc["tui"]["terminal_title"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["project-name", "status", "thread-id"]
    );
    assert_eq!(configure_title(&updated).unwrap(), updated);
    assert!(configure_title("tui = \"invalid\"").is_err());
    assert!(configure_title("[tui]\nterminal_title = false").is_err());
    assert!(configure_title("[tui]\nterminal_title = [1]").is_err());
    let default = configure_title("")
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(
        default["tui"]["terminal_title"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn unix_websocket_snapshot_reads_only_and_handles_pagination_and_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let mut methods = Vec::new();
        loop {
            let Message::Text(text) = socket.read().unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            let method = value["method"].as_str().unwrap();
            methods.push(method.to_owned());
            if method == "initialized" {
                continue;
            }
            let result = match method {
                "initialize" => json!({"userAgent":"test"}),
                "thread/loaded/list" if value["params"]["cursor"].is_null() => {
                    json!({"data":[FIRST],"nextCursor":"next-page"})
                }
                "thread/loaded/list" => json!({"data":[SECOND],"nextCursor":null}),
                "thread/read" => {
                    assert_eq!(value["params"]["includeTurns"], false);
                    json!({"thread":{
                        "id":value["params"]["threadId"], "name":"Actual title",
                        "cwd":"/repo", "preview":"Prompt", "status":{"type":"idle"}
                    }})
                }
                _ => panic!("Unexpected mutating RPC: {method}"),
            };
            socket
                .send(Message::text(
                    json!({"method":"thread/status/changed",
                "params":{"threadId":FIRST,"status":{"type":"idle"}}})
                    .to_string(),
                ))
                .unwrap();
            socket
                .send(Message::text(
                    json!({"id":value["id"],"result":result}).to_string(),
                ))
                .unwrap();
            if method == "thread/read" && value["params"]["threadId"] == SECOND {
                break;
            }
        }
        methods
    });
    let threads = Client::connect(&path).unwrap().snapshot().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[FIRST].name.as_deref(), Some("Actual title"));
    assert_eq!(
        server.join().unwrap(),
        vec![
            "initialize",
            "initialized",
            "thread/loaded/list",
            "thread/read",
            "thread/loaded/list",
            "thread/read"
        ]
    );
}

#[test]
fn missing_daemon_is_optional() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Client::connect(&dir.path().join("missing.sock")).is_err());
}

#[test]
#[ignore = "requires a running local Codex daemon"]
fn live_daemon_snapshot() {
    let path = socket_path().unwrap();
    let threads = Client::connect(&path).unwrap().snapshot().unwrap();
    assert!(!threads.is_empty(), "open a Codex client before this check");
    for (id, thread) in threads {
        assert_eq!(id, thread.id);
        assert!(!thread.cwd.is_empty());
    }
}
