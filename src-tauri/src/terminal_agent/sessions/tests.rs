use super::*;
use std::fs::{File, FileTimes};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const CLAUDE: &str = include_str!("fixtures/claude.jsonl");
const CODEX: &str = include_str!("fixtures/codex.jsonl");
const CLAUDE_ID: &str = "383c4fef-fd6e-452a-9130-bdbdea62bc01";
const CODEX_ID: &str = "876c310b-5293-4557-ae09-11c6cb346802";
const ROOT: &str = "/sample/projects/current";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let home = std::env::temp_dir().join(format!(
            "bento-agent-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&home).unwrap();
        Self(home)
    }

    fn catalog(&self) -> Catalog {
        Catalog::new(Some(self.0.clone()))
    }

    fn write(&self, tool: AgentKind, id: &str, data: &str) -> PathBuf {
        let date = DateTime::from_timestamp_millis(unix_millis(SystemTime::now()) as i64).unwrap();
        let path = match tool {
            AgentKind::Claude => self.0.join(format!(
                ".claude/projects/-folder-with-different-encoding/{id}.jsonl"
            )),
            AgentKind::Codex => self.0.join(format!(
                ".codex/sessions/{}/rollout-fixture-{id}.jsonl",
                date.format("%Y/%m/%d")
            )),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn both_cli_formats_use_cwd_and_extract_title_duration_and_distinct_edits() {
    let fixture = Fixture::new();
    fixture.write(AgentKind::Claude, CLAUDE_ID, CLAUDE);
    fixture.write(AgentKind::Codex, CODEX_ID, CODEX);
    let other_id = "383c4fef-fd6e-452a-9130-bdbdea62bc03";
    fixture.write(
        AgentKind::Claude,
        other_id,
        &CLAUDE
            .replace(CLAUDE_ID, other_id)
            .replace(ROOT, "/sample/projects/current-other"),
    );
    let mut catalog = fixture.catalog();
    catalog.refresh();
    let sessions = catalog.project_sessions(Path::new(ROOT));
    assert_eq!(sessions.len(), 2);
    let [claude, codex] = sessions.as_slice() else {
        unreachable!()
    };
    assert_eq!(claude.reference.tool, AgentKind::Claude);
    assert_eq!(claude.title, "筛选条件进 URL");
    assert_eq!(
        claude.started_at,
        timestamp("2026-11-20T15:40:00Z").unwrap()
    );
    assert_eq!(claude.last_at, timestamp("2026-11-20T16:28:00Z").unwrap());
    assert_eq!(claude.duration_ms, 48 * 60_000);
    assert_eq!(claude.file_count, Some(4));
    assert_eq!(codex.reference.tool, AgentKind::Codex);
    assert_eq!(codex.reference.id, CODEX_ID);
    assert_eq!(codex.title, "导出 CSV 加时间范围");
    assert_eq!(codex.duration_ms, 22 * 60_000);
    assert_eq!(codex.file_count, Some(2));
    assert_eq!(
        catalog
            .project_sessions(Path::new("/sample/projects/current/nested"))
            .len(),
        1
    );
    assert_eq!(
        catalog
            .project_sessions(Path::new("/sample/projects/current-other"))
            .len(),
        1
    );
    assert!(catalog
        .written_since(AgentKind::Codex, Path::new(ROOT), 0, &HashSet::new())
        .is_none());
    assert_eq!(
        catalog.written_since(AgentKind::Codex, &codex.cwd, 0, &HashSet::new()),
        Some(codex.reference.clone())
    );
    assert!(catalog
        .written_since(
            AgentKind::Codex,
            &codex.cwd,
            0,
            &HashSet::from([codex.reference.clone()])
        )
        .is_none());
}

#[test]
fn missing_directories_bad_files_and_old_logs_are_ignored() {
    let fixture = Fixture::new();
    let mut catalog = fixture.catalog();
    catalog.refresh();
    assert!(catalog.project_sessions(Path::new(ROOT)).is_empty());
    let old = fixture.write(AgentKind::Claude, CLAUDE_ID, CLAUDE);
    File::options()
        .write(true)
        .open(old)
        .unwrap()
        .set_times(
            FileTimes::new()
                .set_modified(SystemTime::now() - Duration::from_secs(91 * 24 * 60 * 60)),
        )
        .unwrap();
    fixture.write(
        AgentKind::Codex,
        CODEX_ID,
        "abc\n{\n{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"relative\"}}\n",
    );
    fixture.write(
        AgentKind::Claude,
        "383c4fef-fd6e-452a-9130-bdbdea62bc03",
        "{invalid}\n",
    );
    catalog.refresh();
    assert!(catalog.project_sessions(Path::new(ROOT)).is_empty());
    assert_eq!(catalog.files.len(), 2);
}

#[test]
fn cache_tracks_mtime_and_removals_and_lists_only_the_latest_twenty() {
    let fixture = Fixture::new();
    let mut catalog = fixture.catalog();
    let mut last = PathBuf::new();
    for index in 0..25 {
        let id = format!("383c4fef-fd6e-452a-9130-bdbdea62{index:04}");
        let data = CLAUDE
            .replace(CLAUDE_ID, &id)
            .replace("16:28:00", &format!("16:28:{index:02}"));
        last = fixture.write(AgentKind::Claude, &id, &data);
    }
    catalog.refresh();
    let list = catalog.project_sessions(Path::new(ROOT));
    assert_eq!(list.len(), PROJECT_LIMIT);
    assert!(list[0].reference.id.ends_with("0024"));
    assert!(list[19].reference.id.ends_with("0005"));
    let log = fs::read_to_string(&last)
        .unwrap()
        .replace("筛选条件进 URL", "新的标题进 URL");
    fs::write(&last, log).unwrap();
    let changed = SystemTime::now() + Duration::from_secs(1);
    File::options()
        .write(true)
        .open(&last)
        .unwrap()
        .set_times(FileTimes::new().set_modified(changed))
        .unwrap();
    catalog.refresh();
    assert_eq!(
        catalog.project_sessions(Path::new(ROOT))[0].title,
        "新的标题进 URL"
    );
    fs::remove_file(last).unwrap();
    catalog.refresh();
    assert!(!catalog.project_sessions(Path::new(ROOT))[0]
        .reference
        .id
        .ends_with("0024"));
}

#[test]
fn bounded_samples_keep_tail_time_and_omit_incomplete_file_counts() {
    let fixture = Fixture::new();
    let path = fixture.write(AgentKind::Codex, CODEX_ID, CODEX);
    let mut file = File::options().append(true).open(&path).unwrap();
    for _ in 0..(HEAD_LINES + 2000) {
        writeln!(file, "{}", "invalid bytes".repeat(40)).unwrap();
    }
    writeln!(file, "{{\"timestamp\":\"2026-11-20T14:12:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\"}}}}").unwrap();
    let mut catalog = fixture.catalog();
    catalog.refresh();
    let result = catalog.project_sessions(Path::new(ROOT));
    assert_eq!(result[0].title, "导出 CSV 加时间范围");
    assert_eq!(result[0].duration_ms, 60 * 60_000);
    assert_eq!(result[0].file_count, None);
}

#[test]
fn fallback_user_events_and_legacy_patch_arguments_are_supported() {
    let fixture = Fixture::new();
    let head = CODEX.lines().next().unwrap();
    let message = serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":format!("{}\nextra", "字".repeat(110))}});
    let patch = serde_json::json!({"patch":"*** Begin Patch\n*** Update File: first.txt\n*** Move to: second.txt\n*** End Patch"});
    let call = serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"apply_patch","arguments":patch.to_string()}});
    fixture.write(
        AgentKind::Codex,
        CODEX_ID,
        &format!("{head}\n{message}\n{call}\n"),
    );
    let mut catalog = fixture.catalog();
    catalog.refresh();
    let result = catalog.project_sessions(Path::new(ROOT));
    assert_eq!(result[0].title, format!("{}…", "字".repeat(100)));
    assert_eq!(result[0].file_count, Some(2));
}
