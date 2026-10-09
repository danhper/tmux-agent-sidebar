//! Local Codex daemon observation. The TUI emits its selected thread ID in
//! its terminal title; tmux owns the pane identity. Never infer a binding from
//! cwd, recency, or the daemon's inherited TMUX_PANE.
use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::tmux::PaneStatus;

#[derive(Debug, Clone)]
pub(crate) struct Thread {
    pub id: String,
    pub name: Option<String>,
    pub cwd: String,
    pub preview: String,
    pub status: PaneStatus,
    pub wait_reason: String,
}

pub(crate) type Threads = HashMap<String, Thread>;

pub(crate) fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    Some(codex_home()?.join("app-server-control/app-server-control.sock"))
}

/// Codex 0.161 truncates UUIDs to 29 characters plus "...". Accept only
/// UUID-shaped tokens, and require a unique match against full daemon IDs.
/// The setup command puts thread-id last, after all human-readable items.
pub(crate) fn title_thread_prefix(title: &str) -> Option<&str> {
    let token = title
        .rsplit(" | ")
        .next()?
        .split_whitespace()
        .find(|s| s.len() == 36 || (s.len() == 32 && s.ends_with("...")))?;
    let prefix = token.strip_suffix("...").unwrap_or(token);
    if !prefix.bytes().enumerate().all(|(i, c)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            c == b'-'
        } else {
            c.is_ascii_hexdigit()
        }
    }) {
        return None;
    }
    Some(prefix)
}

pub(crate) fn thread_for_title<'a>(title: &str, threads: &'a Threads) -> Option<&'a Thread> {
    let prefix = title_thread_prefix(title)?;
    let mut matches = threads.values().filter(|t| t.id.starts_with(prefix));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

pub(crate) struct Client {
    socket: WebSocket<UnixStream>,
    next_id: u64,
}

impl Client {
    pub(crate) fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let (socket, _) =
            tungstenite::client("ws://localhost/rpc", stream).map_err(io::Error::other)?;
        let mut client = Self { socket, next_id: 0 };
        client.request(
            "initialize",
            json!({
                "clientInfo": {"name":"tmux_agent_sidebar", "version":crate::VERSION},
                "capabilities": {"experimentalApi":true}
            }),
        )?;
        client.send(json!({"method":"initialized"}))?;
        Ok(client)
    }

    fn send(&mut self, value: Value) -> io::Result<()> {
        self.socket
            .send(Message::text(value.to_string()))
            .map_err(io::Error::other)
    }

    fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Codex daemon RPC timed out",
                ));
            }
            self.socket.get_mut().set_read_timeout(Some(remaining))?;
            let message = self.socket.read().map_err(io::Error::other)?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(io::Error::other(error.to_string()));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| io::Error::other("Missing RPC result"));
        }
    }

    /// Read-only RPCs: do not resume, subscribe, or take ownership of threads
    /// or approvals. Loaded threads are paginated, including idle clients.
    pub(crate) fn snapshot(&mut self) -> io::Result<Threads> {
        let mut threads = Threads::new();
        let mut cursor = Value::Null;
        loop {
            let page = self.request("thread/loaded/list", json!({"cursor":cursor,"limit":100}))?;
            let ids = page
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| io::Error::other("Missing loaded thread IDs"))?;
            for id in ids {
                let Some(id) = id.as_str() else { continue };
                let result =
                    self.request("thread/read", json!({"threadId":id,"includeTurns":false}))?;
                if let Some(thread) = result.get("thread").and_then(parse_thread) {
                    threads.insert(thread.id.clone(), thread);
                }
            }
            let next = page.get("nextCursor").cloned().unwrap_or(Value::Null);
            if next.is_null() {
                break;
            }
            if next == cursor {
                return Err(io::Error::other("Repeated loaded thread cursor"));
            }
            cursor = next;
        }
        Ok(threads)
    }
}

fn parse_thread(value: &Value) -> Option<Thread> {
    let id = value.get("id")?.as_str()?.to_owned();
    let status_value = value.get("status")?;
    let flags = status_value.get("activeFlags").and_then(Value::as_array);
    let waiting = flags.and_then(|flags| {
        flags.iter().find_map(|f| match f.as_str()? {
            "waitingOnApproval" => Some("permission"),
            "waitingOnUserInput" => Some("input"),
            _ => None,
        })
    });
    let status = match status_value.get("type")?.as_str()? {
        "active" if waiting.is_some() => PaneStatus::Waiting,
        "active" => PaneStatus::Running,
        "idle" => PaneStatus::Idle,
        "systemError" => PaneStatus::Error,
        _ => PaneStatus::Unknown,
    };
    Some(Thread {
        id,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")),
        cwd: value.get("cwd")?.as_str()?.to_owned(),
        preview: value
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status,
        wait_reason: waiting.unwrap_or_default().into(),
    })
}

/// Preserve existing title items and comments; put thread-id last so its
/// client-emitted identity is unambiguous in tmux's pane_title.
fn configure_title(config: &str) -> Result<String, String> {
    let mut doc = config
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    if doc.get("tui").is_some_and(|t| t.as_table_like().is_none()) {
        return Err("tui must be a table".into());
    }
    let mut items = match doc.get("tui").and_then(|t| t.get("terminal_title")) {
        None => vec![
            "activity".to_string(),
            "thread-name".into(),
            "project-name".into(),
        ],
        Some(item) => item
            .as_array()
            .ok_or("tui.terminal_title must be an array")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "tui.terminal_title items must be strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    items.retain(|item| !matches!(item.as_str(), "thread-id" | "session-id"));
    items.push("thread-id".into());
    let mut array = toml_edit::Array::new();
    for item in items {
        array.push(item);
    }
    doc["tui"]["terminal_title"] = toml_edit::value(array);
    Ok(doc.to_string())
}

pub(crate) fn setup() -> i32 {
    let Some(home) = codex_home() else {
        eprintln!("Cannot find Codex home");
        return 1;
    };
    let path = home.join("config.toml");
    let result = (|| -> Result<(), String> {
        let config = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.to_string()),
        };
        let updated = configure_title(&config)?;
        if updated != config {
            std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
            let temporary =
                path.with_file_name(format!(".config.sidebar-{}.tmp", std::process::id()));
            let result = (|| -> io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&temporary)?;
                if let Ok(metadata) = std::fs::metadata(&path) {
                    file.set_permissions(metadata.permissions())?;
                }
                file.write_all(updated.as_bytes())?;
                file.sync_all()?;
                if std::fs::read_to_string(&path).unwrap_or_default() != config {
                    return Err(io::Error::other("config.toml changed during setup; retry"));
                }
                std::fs::rename(&temporary, &path)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result.map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!(
                "Enabled Codex daemon pane mapping in {}. Restart Codex clients to apply it.",
                path.display()
            );
            0
        }
        Err(e) => {
            eprintln!("Cannot configure Codex daemon pane mapping: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests;
