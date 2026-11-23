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
            _ => unreachable!("only Claude Code and Codex keep session logs"),
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
    let codex_log = fixture.write(AgentKind::Codex, CODEX_ID, CODEX);
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
    assert!(catalog.reference_for_logs(AgentKind::Codex, &[PathBuf::from(ROOT)]).is_none());
    assert_eq!(
        catalog.reference_for_logs(AgentKind::Codex, std::slice::from_ref(&codex_log)),
        Some(codex.reference.clone())
    );
    assert!(catalog.reference_for_logs(AgentKind::Claude, &[codex_log]).is_none());
}

#[test]
fn simultaneous_processes_in_one_cwd_only_get_their_own_open_log() {
    let fixture = Fixture::new();
    let first = fixture.write(AgentKind::Codex, CODEX_ID, CODEX);
    let second_id = "876c310b-5293-4557-ae09-11c6cb346803";
    let second = fixture.write(AgentKind::Codex, second_id, &CODEX.replace(CODEX_ID, second_id));
    let mut catalog = fixture.catalog();
    catalog.refresh();
    for (path, id) in [(&first, CODEX_ID), (&second, second_id)] {
        assert_eq!(
            catalog.reference_for_logs(AgentKind::Codex, std::slice::from_ref(path)),
            SessionRef::new(AgentKind::Codex, id)
        );
    }
    assert!(catalog.reference_for_logs(AgentKind::Codex, &[]).is_none());
    assert!(catalog.reference_for_logs(AgentKind::Codex, &[first, second]).is_none());
}

#[test]
fn batch_search_uses_registered_roots_and_assigns_nested_sessions_once() {
    let fixture = Fixture::new();
    let claude = fixture.write(AgentKind::Claude, CLAUDE_ID, CLAUDE);
    let duplicate = fixture.0.join(".claude/projects/duplicate");
    fs::create_dir_all(&duplicate).unwrap();
    fs::copy(&claude, duplicate.join(claude.file_name().unwrap())).unwrap();
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
    let sessions = catalog.all_project_sessions([
            ("root", PathBuf::from(ROOT)),
            ("nested", Path::new(ROOT).join("nested")),
            ("invalid", PathBuf::new()),
    ]);
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].project_id, "root");
    assert_eq!(sessions[0].session.reference.id, CLAUDE_ID);
    assert_eq!(sessions[1].project_id, "nested");
    assert_eq!(sessions[1].session.reference.id, CODEX_ID);
    let value = serde_json::to_value(&sessions[1]).unwrap();
    assert_eq!(value["projectId"], "nested");
    assert_eq!(value["id"], CODEX_ID);
    assert_eq!(value["tool"], "codex");
    let other = catalog.all_project_sessions([("other", PathBuf::from("/sample/projects/current-other"))]);
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].session.reference.id, other_id);
    assert!(catalog.all_project_sessions([]).is_empty());
}

#[test]
fn missing_directories_bad_files_and_old_logs_are_ignored() {
    let fixture = Fixture::new();
    let mut catalog = fixture.catalog();
    catalog.refresh();
    assert!(catalog
        .project_sessions(Path::new(ROOT))
        .is_empty());
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
    assert!(catalog
        .project_sessions(Path::new(ROOT))
        .is_empty());
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
    let searchable = catalog.all_project_sessions([("root", PathBuf::from(ROOT))]);
    assert_eq!(searchable.len(), 25);
    assert!(searchable[24].session.reference.id.ends_with("0000"));
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
    assert!(
        !catalog.project_sessions(Path::new(ROOT))[0]
            .reference
            .id
            .ends_with("0024")
    );
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

fn claude_message(kind: &str, time: &str, content: Value) -> Value {
    serde_json::json!({"type": kind, "timestamp": time, "sessionId": CLAUDE_ID, "cwd": ROOT,
        "message": {"role": kind, "content": content}})
}

const START: &str = "2026-11-20T13:12:00.000Z";
const SHORT: &str = "2026-11-20T13:12:59.999Z";
const MINUTE: &str = "2026-11-20T13:13:00.000Z";

fn rows_to_jsonl(rows: &[Value]) -> String {
    rows.iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_rows(tool: AgentKind, rows: &[Value], complete: bool) -> IndexedSession {
    let mut parser = Parser::new(tool, Path::new(CLAUDE_ID));
    for row in rows {
        parser.line(row.to_string().as_bytes());
    }
    parser.finish(complete, 0).unwrap()
}

#[test]
fn claude_short_history_requires_one_user_under_a_minute_and_no_edit_call() {
    use serde_json::json;
    let user = claude_message("user", START, json!("请检查"));
    let read = claude_message(
        "assistant",
        SHORT,
        json!([{"type":"tool_use","name":"Read"}]),
    );
    let tools = claude_message(
        "user",
        SHORT,
        json!([{"type":"tool_result","content":"fixture"}]),
    );
    let mut records = vec![user.clone(), read.clone(), tools];
    let short = parse_rows(AgentKind::Claude, &records, true);
    assert_eq!(short.evidence.user_messages, Some(1));
    assert_eq!(short.evidence.duration_ms, Some(59_999));
    assert!(!short.in_history());
    assert!(parse_rows(AgentKind::Claude, &records, false).in_history());

    records[1]["timestamp"] = json!(MINUTE);
    assert!(parse_rows(AgentKind::Claude, &records, true).in_history());
    records[1] = claude_message("user", SHORT, json!("再问一个问题"));
    assert!(parse_rows(AgentKind::Claude, &records, true).in_history());

    for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
        records[1] = claude_message("assistant", SHORT, json!([{"type":"tool_use","name":tool}]));
        let edited = parse_rows(AgentKind::Claude, &records, true);
        assert!(edited.in_history(), "{tool}");
        assert_eq!(edited.evidence.file_edit, Some(true));
        assert_eq!(edited.summary.file_count, None);
    }

    for (index, field) in [(0, "timestamp"), (1, "message"), (1, "sessionId")] {
        let mut incomplete = vec![user.clone(), read.clone()];
        incomplete[index].as_object_mut().unwrap().remove(field);
        assert!(
            parse_rows(AgentKind::Claude, &incomplete, true).in_history(),
            "{field}"
        );
    }
    records[1] = claude_message("assistant", SHORT, json!([{"type":"tool_use"}]));
    assert!(parse_rows(AgentKind::Claude, &records, true).in_history());
}

#[test]
fn codex_user_event_duplicates_and_distinct_turns_are_counted_and_edits_kept() {
    use serde_json::json;
    let head: Value = serde_json::from_str(CODEX.lines().next().unwrap()).unwrap();
    let user = json!({"timestamp": START, "type":"response_item", "payload":{"type":"message","role":"user",
        "content":[{"type":"input_text","text":"<environment_context>fixture</environment_context>\n检查一下"}]}});
    let event = json!({"timestamp": START, "type":"event_msg","payload":{"type":"user_message","message":"检查一下"}});
    let end = json!({"timestamp": SHORT, "type":"event_msg","payload":{"type":"task_complete"}});
    let mut records = vec![head, user, event, end];
    let short = parse_rows(AgentKind::Codex, &records, true);
    assert_eq!(short.evidence.user_messages, Some(1));
    assert!(!short.in_history());
    records[2]["payload"]["message"] = json!("还有一件事");
    assert_eq!(
        parse_rows(AgentKind::Codex, &records, true)
            .evidence
            .user_messages,
        Some(2)
    );
    records[2]["payload"]["message"] = json!("检查一下");
    records[3]["timestamp"] = json!(MINUTE);
    assert!(parse_rows(AgentKind::Codex, &records, true).in_history());
    records[3]["timestamp"] = json!(SHORT);
    let unclassified =
        json!({"timestamp": SHORT, "type":"response_item","payload":{"type":"custom_tool_call"}});
    records.push(unclassified);
    assert!(parse_rows(AgentKind::Codex, &records, true).in_history());
    for kind in ["custom_tool_call", "function_call"] {
        records[4]["payload"] = json!({"type":kind,"name":"apply_patch"});
        assert_eq!(
            parse_rows(AgentKind::Codex, &records, true)
                .evidence
                .file_edit,
            Some(true)
        );
    }
    records[4] = json!({"timestamp": SHORT,"type":"event_msg","payload":{"type":"patch_apply_end","success":false}});
    assert!(parse_rows(AgentKind::Codex, &records, true).in_history());
}

#[test]
fn both_history_queries_hide_short_sessions() {
    let fixture = Fixture::new();
    let records = [
        claude_message("user", START, serde_json::json!("很短的请求")),
        claude_message(
            "assistant",
            SHORT,
            serde_json::json!([{"type":"text","text":"fixture"}]),
        ),
    ];
    fixture.write(AgentKind::Claude, CLAUDE_ID, &rows_to_jsonl(&records));
    let mut catalog = fixture.catalog();
    catalog.refresh();
    let reference = SessionRef::new(AgentKind::Claude, CLAUDE_ID).unwrap();
    assert!(catalog.find(&reference, Some(Path::new(ROOT))).is_some());
    assert!(catalog.project_sessions(Path::new(ROOT)).is_empty());
    assert!(catalog.all_project_sessions([("root", PathBuf::from(ROOT))]).is_empty());
}

#[test]
fn titles_skip_wrappers_instructions_pastes_images_and_paths_before_the_request() {
    let raw = concat!(
        "# AGENTS.md instructions\n<INSTRUCTIONS>fixture instructions</INSTRUCTIONS>\n",
        "<user_instructions>fixture context</user_instructions>\n<environment_context>fixture env</environment_context>\n",
        "<command-name>/status</command-name><local-command-stdout>fixture output</local-command-stdout>\n",
        "Caveat: fixture caveat\n![preview](fixture.png)\n[Image #1]\n",
        "```js\nconst veryLongSnippet = 1;\n```\n",
        "2026-11-20 13:00:00 ERROR socket closed\nTypeError: sample stack\n    at f (fixture.js:64:8)\n",
        "`/Users/fixture/Projects/sample/a/very/long/configuration.yml`\n",
        "[图片2]  修复\t 登录   回调。下一句不进标题。\nmore"
    );
    let cleaned = title::strip_wrappers(raw).unwrap();
    assert_eq!(
        title::clean_title(&cleaned).as_deref(),
        Some("修复 登录 回调。")
    );
    assert!(title::strip_wrappers(
        "# AGENTS.md instructions\nUnbounded fixture instructions\nsecond instruction"
    )
    .is_none());
    assert_eq!(
        title::clean_title("Export file.json. Keep the rest separate.").as_deref(),
        Some("Export file.json.")
    );
    assert_eq!(
        title::clean_title(&"字".repeat(101)).unwrap(),
        format!("{}…", "字".repeat(100))
    );
}

#[test]
fn skips_empty_cleaned_messages_before_using_the_legacy_title() {
    let records = [
        claude_message("user", START, serde_json::json!("[Image #1]")),
        claude_message("user", SHORT, serde_json::json!("Traceback (most recent call last):\n  File \"test.py\", line 1\n    crash()\nTypeError: fixture")),
        claude_message("user", SHORT, serde_json::json!("完善  CSV 导出")),
    ];
    assert_eq!(
        parse_rows(AgentKind::Claude, &records, true).summary.title,
        "完善 CSV 导出"
    );
    assert_eq!(
        parse_rows(AgentKind::Claude, &records[..1], true)
            .summary
            .title,
        "[Image #1]"
    );
}

#[test]
fn disk_cache_hits_before_refresh_and_reparses_only_changed_mtime_or_size() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let data = fixture.0.join("app-data");
    let claude = fixture.write(AgentKind::Claude, CLAUDE_ID, CLAUDE);
    let codex = fixture.write(AgentKind::Codex, CODEX_ID, CODEX);
    let open = || SharedCatalog::new(Some(fixture.0.clone()), Some(&data));
    let list = |catalog: &Catalog| catalog.project_sessions(Path::new(ROOT));
    let initial = open();
    assert!(list(&initial.snapshot()).is_empty());
    assert_eq!(list(&initial.refresh()).len(), 2);
    let disk = data.join("agent-sessions.json");
    assert_eq!(
        fs::metadata(&disk).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let claude_stamp = fs::metadata(&claude).unwrap().modified().unwrap();
    let updated_claude = CLAUDE.replace("筛选条件进 URL", "过滤规则进 URL");
    assert_eq!(updated_claude.len(), CLAUDE.len());
    fs::write(&claude, &updated_claude).unwrap();
    File::options()
        .write(true)
        .open(&claude)
        .unwrap()
        .set_times(FileTimes::new().set_modified(claude_stamp))
        .unwrap();
    let codex_stamp = fs::metadata(&codex).unwrap().modified().unwrap();
    fs::write(
        &codex,
        CODEX.replace("导出 CSV 加时间范围", "更新导出 CSV 的检索时间范围"),
    )
    .unwrap();
    File::options()
        .write(true)
        .open(&codex)
        .unwrap()
        .set_times(FileTimes::new().set_modified(codex_stamp))
        .unwrap();
    let restarted = open();
    assert_eq!(list(&restarted.snapshot())[0].title, "筛选条件进 URL");
    assert_eq!(list(&restarted.snapshot())[1].title, "导出 CSV 加时间范围");
    let refreshed = list(&restarted.refresh());
    assert_eq!(refreshed[0].title, "筛选条件进 URL");
    assert_eq!(refreshed[1].title, "更新导出 CSV 的检索时间范围");

    File::options()
        .write(true)
        .open(&claude)
        .unwrap()
        .set_times(FileTimes::new().set_modified(claude_stamp + Duration::from_secs(1)))
        .unwrap();
    fs::remove_file(codex).unwrap();
    let refreshed = list(&restarted.refresh());
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].title, "过滤规则进 URL");
    assert_eq!(list(&open().snapshot()).len(), 1);
}

#[test]
fn wrong_version_and_corrupt_disk_cache_rebuild_from_the_source_logs() {
    let fixture = Fixture::new();
    let data = fixture.0.join("app-data");
    fixture.write(AgentKind::Claude, CLAUDE_ID, CLAUDE);
    let open = || SharedCatalog::new(Some(fixture.0.clone()), Some(&data));
    let list = |catalog: &Catalog| catalog.project_sessions(Path::new(ROOT));
    assert_eq!(list(&open().refresh()).len(), 1);
    let disk = data.join("agent-sessions.json");
    let mut wrong: Value = serde_json::from_reader(File::open(&disk).unwrap()).unwrap();
    wrong["version"] = serde_json::json!(0);
    for content in [wrong.to_string(), "{incomplete".into()] {
        fs::write(&disk, content).unwrap();
        let catalog = open();
        assert!(list(&catalog.snapshot()).is_empty());
        assert_eq!(list(&catalog.refresh()).len(), 1);
        assert_eq!(list(&open().snapshot()).len(), 1);
    }
}
