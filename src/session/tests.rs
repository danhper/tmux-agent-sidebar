use super::*;
use std::fs;
use std::io::Write;

struct Fixture {
    dir: tempfile::TempDir,
    scanner: SessionNameScanner,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            scanner: SessionNameScanner::default(),
        }
    }

    fn write(&self, path: &str, content: &str) -> PathBuf {
        let path = self.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn scan(&mut self) -> HashMap<String, String> {
        self.scanner.scan_paths(
            &self.dir.path().join("claude"),
            &self.dir.path().join("codex"),
        )
    }
}

#[test]
fn missing_and_malformed_sources_are_optional() {
    let mut fixture = Fixture::new();
    assert!(fixture.scan().is_empty());
    fixture.write("claude/sessions/bad.json", "not json");
    fixture.write("claude/sessions/no-id.json", r#"{"name":"orphan"}"#);
    fixture.write(
        "claude/sessions/blank.json",
        r#"{"sessionId":" ","name":"x"}"#,
    );
    fixture.write("claude/sessions/no-name.json", r#"{"sessionId":"unnamed"}"#);
    fixture.write(
        "claude/sessions/blank-name.json",
        r#"{"sessionId":"blank","name":"  "}"#,
    );
    fixture.write(
        "claude/sessions/ignored.txt",
        r#"{"sessionId":"ignored","name":"x"}"#,
    );
    fixture.write("codex/session_index.jsonl", "not json\n");
    assert!(fixture.scan().is_empty());
}

#[test]
fn claude_prefers_custom_then_generated_then_live_name() {
    let mut fixture = Fixture::new();
    fixture.write(
        "claude/sessions/1.json",
        r#"{"sessionId":"live","name":"fallback"}"#,
    );
    fixture.write(
        "claude/sessions/2.json",
        r#"{"sessionId":"generated","name":"derived-name"}"#,
    );
    fixture.write(
        "claude/sessions/3.json",
        r#"{"sessionId":"custom","name":"derived-name"}"#,
    );
    fixture.write("claude/sessions/4.json", r#"{"sessionId":"no-live-name"}"#);
    fixture.write(
        "claude/projects/repo/generated.jsonl",
        "{\"type\":\"ai-title\",\"sessionId\":\"generated\",\"aiTitle\":\"Fix parser\"}\n",
    );
    fixture.write("claude/projects/repo/custom.jsonl", concat!(
        "{\"type\":\"custom-title\",\"sessionId\":\"custom\",\"customTitle\":\"My chosen name\"}\n",
        "{\"type\":\"ai-title\",\"sessionId\":\"custom\",\"aiTitle\":\"Generated later\"}\n"
    ));
    fixture.write(
        "claude/projects/repo/no-live-name.jsonl",
        "{\"type\":\"ai-title\",\"sessionId\":\"no-live-name\",\"aiTitle\":\"New task\"}\n",
    );
    fixture.write(
        "claude/projects/repo/inactive.jsonl",
        "{\"type\":\"ai-title\",\"sessionId\":\"inactive\",\"aiTitle\":\"Old task\"}\n",
    );
    assert_eq!(
        fixture.scan(),
        HashMap::from([
            ("live".into(), "fallback".into()),
            ("generated".into(), "Fix parser".into()),
            ("custom".into(), "My chosen name".into()),
            ("no-live-name".into(), "New task".into()),
        ])
    );
    assert_eq!(fixture.scanner.transcripts.len(), 3);
}

#[test]
fn claude_reads_appended_titles_and_retries_partial_records() {
    let mut fixture = Fixture::new();
    fixture.write(
        "claude/sessions/1.json",
        r#"{"sessionId":"thread","name":"fallback"}"#,
    );
    let path = fixture.write(
        "claude/projects/repo/thread.jsonl",
        concat!(
            "{\"type\":\"ai-title\",\"sessionId\":\"thread\",\"aiTitle\":\"First title\"}\n",
            "{\"type\":\"ai-title\",\"sessionId\":\"thread\",\"aiTitle\":\"New"
        ),
    );
    assert_eq!(fixture.scan()["thread"], "First title");
    let offset = fixture.scanner.transcripts[&path].offset;
    assert_eq!(fixture.scan()["thread"], "First title");
    assert_eq!(fixture.scanner.transcripts[&path].offset, offset);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, " title\"}}").unwrap();
    assert_eq!(fixture.scan()["thread"], "New title");
    assert!(fixture.scanner.transcripts[&path].offset > offset);
    // Malformed, blank, and unrelated title records must not erase a title.
    writeln!(file, "not json").unwrap();
    writeln!(
        file,
        r#"{{"type":"ai-title","sessionId":"thread","aiTitle":" "}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"ai-title","sessionId":"other","aiTitle":"Wrong thread"}}"#
    )
    .unwrap();
    assert_eq!(fixture.scan()["thread"], "New title");
}

#[test]
fn claude_resets_truncated_transcripts_and_drops_inactive_cache() {
    let mut fixture = Fixture::new();
    let live = fixture.write(
        "claude/sessions/1.json",
        r#"{"sessionId":"thread","name":"fallback"}"#,
    );
    let transcript = fixture.write(
        "claude/projects/repo/thread.jsonl",
        "{\"type\":\"ai-title\",\"sessionId\":\"thread\",\"aiTitle\":\"Original long title\"}\n",
    );
    assert_eq!(fixture.scan()["thread"], "Original long title");
    fs::write(
        &transcript,
        "{\"type\":\"ai-title\",\"sessionId\":\"thread\",\"aiTitle\":\"Short\"}\n",
    )
    .unwrap();
    assert_eq!(fixture.scan()["thread"], "Short");
    fs::remove_file(live).unwrap();
    assert!(fixture.scan().is_empty());
    assert!(fixture.scanner.transcripts.is_empty());
}

#[test]
fn codex_latest_valid_title_wins_and_missing_titles_keep_fallback() {
    let mut fixture = Fixture::new();
    fixture.write(
        "codex/session_index.jsonl",
        concat!(
            "{\"id\":\"one\",\"thread_name\":\"Old name\",\"updated_at\":\"2026-01-01\"}\n",
            "malformed\n",
            "{\"id\":\"two\",\"thread_name\":\"Other task\"}\n",
            "{\"id\":\"one\",\"thread_name\":\"New name\"}\n",
            "{\"id\":\"one\",\"thread_name\":\" \"}\n",
            "{\"id\":\"missing-title\"}\n",
            "{\"thread_name\":\"missing ID\"}\n"
        ),
    );
    assert_eq!(
        fixture.scan(),
        HashMap::from([
            ("one".into(), "New name".into()),
            ("two".into(), "Other task".into()),
        ])
    );
    fixture.write(
        "codex/session_index.jsonl",
        "{\"id\":\"one\",\"thread_name\":\"Renamed\"}\n",
    );
    assert_eq!(fixture.scan()["one"], "Renamed");
}

#[test]
fn titles_are_trimmed_and_kept_on_one_terminal_row() {
    let value = serde_json::json!({"title": "  Fix\n parser\t bug\u{0007}  "});
    assert_eq!(text(&value, "title"), Some("Fix parser bug".into()));
    assert_eq!(
        text(&serde_json::json!({"title":"\u{0007}"}), "title"),
        None
    );
    assert_eq!(text(&serde_json::json!({"title":42}), "title"), None);
}
