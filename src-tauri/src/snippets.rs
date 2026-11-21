use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Snippet {
    id: i64,
    name: String,
    command: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Change {
    Add {
        name: String,
        command: String,
    },
    Edit {
        id: i64,
        name: String,
        command: String,
    },
    Delete {
        id: i64,
    },
}

pub fn create_table(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_snippets (
        id INTEGER PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        name TEXT NOT NULL, command TEXT NOT NULL
    ); CREATE INDEX IF NOT EXISTS snippets_project ON project_snippets(project_id);",
    )
}

fn read(
    connection: &Connection,
    project_id: Option<&str>,
) -> rusqlite::Result<std::collections::BTreeMap<String, Vec<Snippet>>> {
    let mut statement = connection.prepare(
        "SELECT project_id,id,name,command FROM project_snippets WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id",
    )?;
    let rows = statement.query_map([project_id], |row| {
        Ok((
            row.get(0)?,
            Snippet {
                id: row.get(1)?,
                name: row.get(2)?,
                command: row.get(3)?,
            },
        ))
    })?;
    let mut projects = std::collections::BTreeMap::<String, Vec<Snippet>>::new();
    for row in rows {
        let (project_id, snippet) = row?;
        projects.entry(project_id).or_default().push(snippet);
    }
    Ok(projects)
}

fn validated<'a>(name: &'a str, command: &str) -> Result<(&'a str, String), String> {
    let name = name.trim();
    let command = command.trim();
    if name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err("名称最多 40 个字符，不能含控制字符".into());
    }
    if command.is_empty()
        || command.len() > 4096
        || command
            .chars()
            .any(|char| char.is_control() && !matches!(char, '\n' | '\r' | '\t'))
    {
        return Err("请输入命令，最多 4096 字节，且不能含终端控制序列".into());
    }
    Ok((name, command.replace("\r\n", "\n").replace('\r', "\n")))
}

fn change(
    connection: &mut Connection,
    project_id: &str,
    change: Change,
) -> Result<Vec<Snippet>, String> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err("未找到该项目".into());
    }
    let changed = match change {
        Change::Add { name, command } => {
            let (name, command) = validated(&name, &command)?;
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM project_snippets WHERE project_id=?1",
                    [project_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if count >= 20 {
                return Err("每个项目最多 20 条常用命令".into());
            }
            tx.execute(
                "INSERT INTO project_snippets(project_id,name,command) VALUES (?1,?2,?3)",
                params![project_id, name, command],
            )
        }
        Change::Edit { id, name, command } => {
            let (name, command) = validated(&name, &command)?;
            tx.execute(
                "UPDATE project_snippets SET name=?1,command=?2 WHERE id=?3 AND project_id=?4",
                params![name, command, id, project_id],
            )
        }
        Change::Delete { id } => tx.execute(
            "DELETE FROM project_snippets WHERE id=?1 AND project_id=?2",
            params![id, project_id],
        ),
    }
    .map_err(|error| error.to_string())?;
    if changed == 0 {
        return Err("常用命令已被移除".into());
    }
    let snippets = read(&tx, Some(project_id))
        .map_err(|error| error.to_string())?
        .remove(project_id)
        .unwrap_or_default();
    tx.commit().map_err(|error| error.to_string())?;
    Ok(snippets)
}

#[tauri::command]
pub fn get_snippets() -> Result<std::collections::BTreeMap<String, Vec<Snippet>>, String> {
    read(&crate::open_project_db()?, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn change_snippet(project_id: String, operation: Change) -> Result<Vec<Snippet>, String> {
    change(&mut crate::open_project_db()?, &project_id, operation)
}

#[derive(Serialize)]
pub struct Suggestion {
    name: String,
    command: String,
}

fn suggestions(directory: &Path) -> Vec<Suggestion> {
    let manifest = directory.join("package.json");
    if !std::fs::metadata(&manifest).is_ok_and(|meta| meta.len() <= 1024 * 1024) {
        return Vec::new();
    }
    let Some(value) = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|scripts| scripts.as_object()) else {
        return Vec::new();
    };
    let manager = value
        .get("packageManager")
        .and_then(|value| value.as_str())
        .and_then(|name| name.split('@').next())
        .filter(|name| ["pnpm", "npm", "yarn", "bun"].contains(name))
        .unwrap_or_else(|| {
            if directory.join("pnpm-lock.yaml").is_file() {
                "pnpm"
            } else if directory.join("yarn.lock").is_file() {
                "yarn"
            } else if directory.join("bun.lock").is_file() || directory.join("bun.lockb").is_file()
            {
                "bun"
            } else {
                "npm"
            }
        });
    [
        "lint",
        "test",
        "test:watch",
        "db:migrate",
        "migrate",
        "seed",
    ]
    .into_iter()
    .filter(|name| {
        scripts
            .get(*name)
            .and_then(|script| script.as_str())
            .is_some_and(|script| !script.trim().is_empty())
    })
    .map(|name| Suggestion {
        name: name.into(),
        command: format!("{manager} run {name}"),
    })
    .collect()
}

#[tauri::command]
pub fn suggest_snippets(project_id: String) -> Result<Vec<Suggestion>, String> {
    let project = crate::get_projects_sync()?
        .projects
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or("未找到该项目")?;
    Ok(suggestions(Path::new(&crate::expand_tilde(&project.path))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_create_edit_delete_are_project_scoped_and_keep_optional_names() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        for id in ["a", "b"] {
            conn.execute(
                "INSERT INTO projects(id,name,path,start_command) VALUES(?1,?1,'/tmp','echo')",
                [id],
            )
            .unwrap();
        }
        let created = change(
            &mut conn,
            "a",
            Change::Add {
                name: String::new(),
                command: "  pnpm lint  ".into(),
            },
        )
        .unwrap();
        assert_eq!(created[0].name, "");
        assert_eq!(created[0].command, "pnpm lint");
        let id = created[0].id;
        assert!(change(&mut conn, "b", Change::Delete { id }).is_err());
        assert!(change(
            &mut conn,
            "a",
            Change::Edit {
                id,
                name: String::new(),
                command: "\x1b[31m".into()
            }
        )
        .is_err());
        let edited = change(
            &mut conn,
            "a",
            Change::Edit {
                id,
                name: "测试".into(),
                command: "pnpm test".into(),
            },
        )
        .unwrap();
        assert_eq!(
            edited[0],
            Snippet {
                id,
                name: "测试".into(),
                command: "pnpm test".into()
            }
        );
        assert_eq!(read(&conn, None).unwrap().get("a"), Some(&edited));
        assert!(change(&mut conn, "a", Change::Delete { id })
            .unwrap()
            .is_empty());
        assert!(read(&conn, None).unwrap().is_empty());
    }
}
