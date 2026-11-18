use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub collapsed: bool,
}

pub fn read_groups(conn: &Connection) -> Result<Vec<ProjectGroup>, String> {
    let mut statement = conn.prepare("SELECT id,name,sort_order,collapsed FROM groups ORDER BY sort_order,id")
        .map_err(|err| err.to_string())?;
    let rows = statement.query_map([], |row| Ok(ProjectGroup {
        id: row.get(0)?, name: row.get(1)?, sort_order: row.get(2)?, collapsed: row.get(3)?,
    })).map_err(|err| err.to_string())?;
    rows.collect::<rusqlite::Result<_>>().map_err(|err| err.to_string())
}

pub fn validate_group(conn: &Connection, id: Option<&str>) -> Result<(), String> {
    if let Some(id) = id {
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM groups WHERE id=?1)", [id], |row| row.get(0))
            .map_err(|err| err.to_string())?;
        if !exists { return Err("分组已不存在，请刷新后重试".into()); }
    }
    Ok(())
}

fn group_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() { return Err("请输入分组名称".into()); }
    if name.chars().count() > 40 { return Err("分组名称最多 40 个字符".into()); }
    Ok(name)
}

pub fn insert_group(conn: &Connection, name: &str) -> Result<ProjectGroup, String> {
    let name = group_name(name)?;
    let transaction = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|err| err.to_string())?;
    let groups = read_groups(&transaction)?;
    if groups.iter().any(|group| group.name == name) { return Err("已有同名分组".into()); }
    let mut suffix = 1;
    let id = loop {
        let id = format!("group-{suffix}");
        if groups.iter().all(|group| group.id != id) { break id; }
        suffix += 1;
    };
    let sort_order = groups.iter().map(|group| group.sort_order).max().unwrap_or(-1) + 1;
    transaction.execute("INSERT INTO groups (id,name,sort_order) VALUES (?1,?2,?3)", params![id, name, sort_order])
        .map_err(|err| format!("新建分组失败：{err}"))?;
    transaction.commit().map_err(|err| err.to_string())?;
    Ok(ProjectGroup { id, name: name.into(), sort_order, collapsed: false })
}

pub fn update_group_row(conn: &Connection, id: &str, name: Option<&str>, collapsed: Option<bool>) -> Result<(), String> {
    let name = name.map(group_name).transpose()?;
    if let Some(name) = name {
        let duplicate: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM groups WHERE name=?1 AND id!=?2)", params![name, id], |row| row.get(0))
            .map_err(|err| err.to_string())?;
        if duplicate { return Err("已有同名分组".into()); }
    }
    let count = conn.execute("UPDATE groups SET name=COALESCE(?1,name), collapsed=COALESCE(?2,collapsed) WHERE id=?3", params![name, collapsed, id])
        .map_err(|err| format!("更新分组失败：{err}"))?;
    if count == 0 { return Err("分组已不存在，请刷新后重试".into()); }
    Ok(())
}

pub fn delete_group_row(conn: &mut Connection, id: &str) -> Result<(), String> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    validate_group(&transaction, Some(id))?;
    transaction.execute("UPDATE projects SET group_id=NULL, updated_at=CURRENT_TIMESTAMP WHERE group_id=?1", [id]).map_err(|err| err.to_string())?;
    transaction.execute("DELETE FROM groups WHERE id=?1", [id]).map_err(|err| err.to_string())?;
    transaction.commit().map_err(|err| err.to_string())
}

pub fn reorder_group_rows(conn: &mut Connection, ids: &[String]) -> Result<(), String> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    let current: HashSet<_> = read_groups(&transaction)?.into_iter().map(|group| group.id).collect();
    let requested: HashSet<_> = ids.iter().cloned().collect();
    if ids.len() != current.len() || requested != current { return Err("分组列表已变化，请刷新后重试".into()); }
    for (index, id) in ids.iter().enumerate() {
        transaction.execute("UPDATE groups SET sort_order=?1 WHERE id=?2", params![index as i64, id]).map_err(|err| err.to_string())?;
    }
    transaction.commit().map_err(|err| err.to_string())
}

pub fn move_project_row(conn: &mut Connection, id: &str, group_id: Option<&str>, ids: &[String]) -> Result<(), String> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    validate_group(&transaction, group_id)?;
    crate::projects::write_project_order(&transaction, ids)?;
    let count = transaction.execute("UPDATE projects SET group_id=?1, updated_at=CURRENT_TIMESTAMP WHERE id=?2", params![group_id, id])
        .map_err(|err| err.to_string())?;
    if count == 0 { return Err("未找到项目".into()); }
    transaction.commit().map_err(|err| err.to_string())
}

pub fn set_project_flags_row(conn: &Connection, id: &str, pinned: Option<bool>, archived: Option<bool>) -> Result<(), String> {
    let count = conn.execute("UPDATE projects SET pinned=COALESCE(?1,pinned), archived=COALESCE(?2,archived), updated_at=CURRENT_TIMESTAMP WHERE id=?3", params![pinned, archived, id])
        .map_err(|err| err.to_string())?;
    if count == 0 { return Err("未找到项目".into()); }
    Ok(())
}

#[tauri::command]
pub fn add_group(name: String) -> Result<ProjectGroup, String> { insert_group(&crate::open_project_db()?, &name) }

#[tauri::command]
pub fn update_group(id: String, name: Option<String>, collapsed: Option<bool>) -> Result<(), String> {
    update_group_row(&crate::open_project_db()?, &id, name.as_deref(), collapsed)
}

#[tauri::command]
pub fn delete_group(id: String) -> Result<(), String> { delete_group_row(&mut crate::open_project_db()?, &id) }

#[tauri::command]
pub fn reorder_groups(ids: Vec<String>) -> Result<(), String> { reorder_group_rows(&mut crate::open_project_db()?, &ids) }

#[tauri::command]
pub fn move_project(id: String, group_id: Option<String>, ids: Vec<String>) -> Result<(), String> {
    move_project_row(&mut crate::open_project_db()?, &id, group_id.as_deref(), &ids)
}

#[tauri::command]
pub fn set_project_flags(id: String, pinned: Option<bool>, archived: Option<bool>) -> Result<(), String> {
    set_project_flags_row(&crate::open_project_db()?, &id, pinned, archived)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_db(&conn).unwrap();
        conn.execute_batch("INSERT INTO projects (id,name,path,start_command,sort_order) VALUES ('a','A','/tmp','sleep 1',0),('b','B','/tmp','sleep 1',1)").unwrap();
        conn
    }
    fn ids() -> Vec<String> { vec!["b".into(), "a".into()] }

    #[test]
    fn migration_keeps_legacy_fields_and_defaults_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY,name TEXT,display_name TEXT,description TEXT,path TEXT,port INTEGER,start_command TEXT,type TEXT,sort_order INTEGER,env TEXT,commands TEXT,auto_open INTEGER,created_at TEXT,updated_at TEXT);
            INSERT INTO projects VALUES ('legacy','原项目','原显示名','描述','/tmp',5173,'pnpm dev','web',7,'[]','[]',1,'old','unchanged')").unwrap();
        crate::init_db(&conn).unwrap();
        crate::init_db(&conn).unwrap();
        let projects = crate::read_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.display_name.as_deref(), Some("原显示名"));
        assert!(project.auto_open && project.group_id.is_none() && !project.pinned && !project.archived);
        assert!(read_groups(&conn).unwrap().is_empty());
        assert_eq!(conn.query_row("SELECT sort_order,updated_at FROM projects", [], |row| Ok((row.get::<_, i64>(0)?,row.get::<_, String>(1)?))).unwrap(), (7,"unchanged".into()));
    }

    #[test]
    fn group_crud_collapse_sort_and_delete_preserve_projects_and_flags() {
        let mut conn = database();
        let work = insert_group(&conn, "  工作  ").unwrap();
        let personal = insert_group(&conn, "个人").unwrap();
        assert!(insert_group(&conn, "工作").is_err());
        assert!(insert_group(&conn, " ").is_err());
        assert!(insert_group(&conn, &"中".repeat(41)).is_err());
        update_group_row(&conn, &work.id, Some("团队"), Some(true)).unwrap();
        reorder_group_rows(&mut conn, &[personal.id.clone(),work.id.clone()]).unwrap();
        assert!(reorder_group_rows(&mut conn, &[work.id.clone(),work.id.clone()]).is_err());
        let groups = read_groups(&conn).unwrap();
        assert_eq!(groups[0].id, personal.id);
        assert!(groups[1].collapsed);
        assert_eq!(groups[1].name, "团队");
        assert!(update_group_row(&conn, &work.id, Some("个人"), None).is_err());
        move_project_row(&mut conn,"a",Some(&work.id),&ids()).unwrap();
        set_project_flags_row(&conn,"a",Some(true),Some(true)).unwrap();
        delete_group_row(&mut conn,&work.id).unwrap();
        let projects = crate::read_projects(&conn).unwrap();
        assert_eq!(projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["b","a"]);
        assert!(projects[1].group_id.is_none() && projects[1].pinned && projects[1].archived);
        assert!(delete_group_row(&mut conn,&work.id).is_err());
    }

    #[test]
    fn project_moves_are_atomic_and_flags_round_trip_independently() {
        let mut conn = database();
        let group = insert_group(&conn,"工作").unwrap();
        assert!(move_project_row(&mut conn,"a",Some(&group.id),&["a".into(),"a".into()]).is_err());
        assert!(move_project_row(&mut conn,"a",Some("missing"),&ids()).is_err());
        assert!(crate::read_projects(&conn).unwrap()[0].group_id.is_none());
        move_project_row(&mut conn,"a",Some(&group.id),&ids()).unwrap();
        set_project_flags_row(&conn,"a",Some(true),None).unwrap();
        set_project_flags_row(&conn,"a",None,Some(true)).unwrap();
        crate::init_db(&conn).unwrap();
        let mut project = crate::read_projects(&conn).unwrap().remove(1);
        assert_eq!(project.group_id.as_deref(),Some(group.id.as_str()));
        assert!(project.pinned && project.archived);
        // Editing ordinary configuration keeps the independent sidebar flags.
        let draft = serde_json::from_value(serde_json::json!({"name":"Edited","path":"/tmp","startCommand":"sleep 1","type":"app","groupId":group.id})).unwrap();
        project = crate::projects::update_project_row(&conn,"a",draft).unwrap();
        assert!(project.pinned && project.archived && project.group_id.is_some());
        set_project_flags_row(&conn,"a",None,Some(false)).unwrap();
        set_project_flags_row(&conn,"a",Some(false),None).unwrap();
        move_project_row(&mut conn,"a",None,&ids()).unwrap();
        project = crate::read_projects(&conn).unwrap().remove(1);
        assert!(!project.pinned && !project.archived && project.group_id.is_none());
        assert!(set_project_flags_row(&conn,"missing",Some(true),None).is_err());
    }

    #[test]
    fn draft_group_membership_is_validated_on_both_insert_and_edit() {
        let conn = database();
        let group = insert_group(&conn, "工作").unwrap();
        let draft = |group_id: &str| serde_json::from_value(serde_json::json!({
            "name":"New app","path":"/tmp","type":"app","startCommand":"sleep 1","groupId":group_id
        })).unwrap();
        assert!(crate::projects::insert_project(&conn, draft("missing")).is_err());
        let project = crate::projects::insert_project(&conn, draft(&group.id)).unwrap();
        assert_eq!(project.group_id.as_deref(), Some(group.id.as_str()));
        assert!(crate::projects::update_project_row(&conn, &project.id, draft("missing")).is_err());
        let saved = crate::read_projects(&conn).unwrap().into_iter().find(|p| p.id == project.id).unwrap();
        assert_eq!(saved.group_id, Some(group.id));
    }
}
