use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

const FONT_SIZE: &str = "terminal.fontSize";
const DEFAULT_APPLIED: &str = "autostart.defaultApplied";

#[derive(Debug, Serialize, PartialEq)]
pub struct Settings {
    #[serde(rename = "terminal.fontSize")]
    pub terminal_font_size: u8,
    #[serde(rename = "autostart.defaultApplied")]
    pub autostart_default_applied: bool,
}

pub fn read(conn: &Connection) -> Result<Settings, String> {
    let saved: Option<String> = conn.query_row(
        "SELECT value FROM settings WHERE key = ?1", [FONT_SIZE], |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;
    let terminal_font_size = saved.and_then(|value| value.parse::<u8>().ok())
        .filter(|size| (12..=20).contains(size)).unwrap_or(14);
    Ok(Settings { terminal_font_size, autostart_default_applied: default_applied(conn)? })
}

pub fn default_applied(conn: &Connection) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)", [DEFAULT_APPLIED], |row| row.get(0))
        .map_err(|error| error.to_string())
}

pub fn record_default_applied(conn: &Connection) -> Result<(), String> {
    save(conn, DEFAULT_APPLIED, "true")
}

fn save(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
        params![key, value],
    ).map(|_| ()).map_err(|error| format!("保存设置失败：{error}"))
}

pub fn set(conn: &Connection, key: &str, value: Value) -> Result<(), String> {
    match key {
        FONT_SIZE => {
            let size = value.as_u64().filter(|size| (12..=20).contains(size))
                .ok_or("终端字号必须是 12–20 的整数")?;
            save(conn, key, &size.to_string())
        }
        DEFAULT_APPLIED => Err("该设置由 Bento 自动管理".into()),
        _ => Err("未知设置".into()),
    }
}

#[tauri::command]
pub fn get_settings() -> Result<Settings, String> {
    read(&crate::open_project_db()?)
}

#[tauri::command]
pub fn set_setting(key: String, value: Value) -> Result<(), String> {
    set(&crate::open_project_db()?, &key, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project_rows(conn: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        let mut stmt = conn.prepare("SELECT * FROM projects ORDER BY id").unwrap();
        stmt.query_map([], |row| (0..row.as_ref().column_count()).map(|i| row.get(i)).collect())
            .unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn settings_migration_preserves_all_seven_existing_project_rows() {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        conn.execute("DROP TABLE settings", []).unwrap();
        for i in 0..7 {
            conn.execute("INSERT INTO projects (id,name,path,port,start_command,sort_order,env,commands,created_at,updated_at)
                VALUES (?1,?2,'/tmp/项目',?3,'pnpm dev',?4,'[]','[]','2025-01-02','2025-03-04')",
                params![format!("p{i}"), format!("项目 {i}"), 6000 + i, i]).unwrap();
        }
        let before = project_rows(&conn);
        crate::init_db(&conn).unwrap();
        crate::init_db(&conn).unwrap();
        assert_eq!(project_rows(&conn), before);
        assert_eq!(read(&conn).unwrap(), Settings { terminal_font_size: 14, autostart_default_applied: false });
        set(&conn, FONT_SIZE, json!(18)).unwrap();
        assert_eq!(read(&conn).unwrap().terminal_font_size, 18);
        assert_eq!(project_rows(&conn), before);
    }

    #[test]
    fn fresh_settings_validate_known_user_keys_and_persist_defaults_safely() {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        assert_eq!(read(&conn).unwrap().terminal_font_size, 14);
        for value in [json!(11), json!(21), json!(-1), json!(14.5), json!("18"), json!(true), Value::Null] {
            assert!(set(&conn, FONT_SIZE, value).is_err());
        }
        assert!(set(&conn, "unknown", json!(18)).is_err());
        assert!(set(&conn, DEFAULT_APPLIED, json!(true)).is_err());
        for size in [12, 18, 20] {
            set(&conn, FONT_SIZE, json!(size)).unwrap();
            assert_eq!(read(&conn).unwrap().terminal_font_size, size);
        }
        conn.execute("UPDATE settings SET value = 'invalid' WHERE key = ?1", [FONT_SIZE]).unwrap();
        assert_eq!(read(&conn).unwrap().terminal_font_size, 14);
        record_default_applied(&conn).unwrap();
        assert!(read(&conn).unwrap().autostart_default_applied);
    }
}
