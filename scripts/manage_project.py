#!/usr/bin/env python3
"""
Bento SQLite Database Project Manager CLI for AI and Developers.

Database: ~/.apphub/apphub.db
Usage:
    python3 manage_project.py list [--group <name>]
    python3 manage_project.py add --name <name> --path <path> (--command <command> | --no-run) [--port <port>] [--type <web|app>] [--display <display>] [--desc <desc>] [--group <name>]
    python3 manage_project.py update --id <id> [--name <name>] [--path <path>] [--port <port>] [--command <command>] [--type <web|app>] [--group <name>] [--run | --no-run]
    python3 manage_project.py delete <id>
    python3 manage_project.py check-port <port>
"""

import sys
import os
import sqlite3
import argparse
import json
import shutil
from pathlib import Path

DB_PATH = Path.home() / ".apphub" / "apphub.db"
OLD_DB_PATH = Path.home() / ".fronthub" / "fronthub.db"

def get_connection():
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    # Automatic migration from old ~/.fronthub/fronthub.db if new DB doesn't exist
    if not DB_PATH.exists() and OLD_DB_PATH.exists():
        shutil.copy2(OLD_DB_PATH, DB_PATH)

    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row
    init_db(conn)
    return conn

def init_db(conn):
    with conn:
        conn.execute("""
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                display_name TEXT,
                description TEXT,
                path TEXT NOT NULL,
                port INTEGER UNIQUE,
                start_command TEXT NOT NULL,
                type TEXT DEFAULT 'web',
                sort_order INTEGER,
                env TEXT,
                commands TEXT,
                auto_open INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
        """)
        # Migration: ensure 'type' column exists
        cur = conn.cursor()
        cur.execute("PRAGMA table_info(projects)")
        cols = [c[1] for c in cur.fetchall()]
        if "type" not in cols:
            conn.execute("ALTER TABLE projects ADD COLUMN type TEXT DEFAULT 'web'")
        conn.execute("""CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL,
            collapsed INTEGER NOT NULL DEFAULT 0
        )""")
        for column, definition in [
            ("group_id", "TEXT REFERENCES groups(id) ON DELETE SET NULL"),
            ("pinned", "INTEGER NOT NULL DEFAULT 0"),
            ("archived", "INTEGER NOT NULL DEFAULT 0"),
            ("services", "TEXT NOT NULL DEFAULT '[]'"),
            ("inject_ports", "INTEGER NOT NULL DEFAULT 1"),
            ("runnable", "INTEGER NOT NULL DEFAULT 1"),
        ]:
            if column not in cols:
                conn.execute(f"ALTER TABLE projects ADD COLUMN {column} {definition}")
        # sort_order / env / commands / auto_open are migrated by the Bento app itself,
        # which also preserves the existing sidebar order. An empty database stays empty.

def project_columns(conn):
    return {c[1] for c in conn.execute("PRAGMA table_info(projects)").fetchall()}

def resolve_group(conn, name):
    name = name.strip()
    if not name:
        return None
    if len(name) > 40:
        print("❌ 错误：分组名称最多 40 个字符。", file=sys.stderr)
        sys.exit(1)
    existing = conn.execute("SELECT id FROM groups WHERE name=? ORDER BY sort_order,id LIMIT 1", (name,)).fetchone()
    if existing:
        return existing['id']
    suffix = 1
    while conn.execute("SELECT 1 FROM groups WHERE id=?", (f"group-{suffix}",)).fetchone():
        suffix += 1
    group_id = f"group-{suffix}"
    conn.execute("INSERT INTO groups (id,name,sort_order) VALUES (?,?,(SELECT COALESCE(MAX(sort_order),-1)+1 FROM groups))", (group_id, name))
    return group_id

def port_project(conn, port, exclude_id=None):
    return conn.execute("""
        SELECT p.id, p.name, p.display_name FROM projects p
        WHERE (?2 IS NULL OR p.id != ?2) AND (p.port = ?1 OR EXISTS (
            SELECT 1 FROM json_tree(CASE WHEN json_valid(p.services) THEN p.services ELSE '[]' END) j
            WHERE j.key = 'port' AND j.type = 'integer' AND j.atom = ?1
        )) ORDER BY p.id LIMIT 1
    """, (port, exclude_id)).fetchone()

def ensure_unique_port(conn, port, exclude_id=None):
    if port is None:
        return
    if not 1 <= port <= 65535:
        print("❌ 错误：端口必须是 1–65535 之间的整数。", file=sys.stderr)
        sys.exit(1)
    conflict = port_project(conn, port, exclude_id)
    if conflict:
        display = conflict['display_name'] or conflict['name']
        print(f"❌ 错误：端口 {port} 已被项目「{display}」(ID: {conflict['id']}) 占用！各项目端口必须唯一，请更换端口。", file=sys.stderr)
        sys.exit(1)

def cmd_list(args):
    conn = get_connection()
    cur = conn.cursor()
    # Use the saved manual project order; the UI also partitions pins/groups/archives.
    # Databases the app hasn't migrated yet keep the old port order.
    order = "p.sort_order IS NULL, p.sort_order, p.created_at, p.id" if "sort_order" in project_columns(conn) else "p.port ASC"
    where = "" if args.group is None else "WHERE COALESCE(g.name,'')=?"
    cur.execute(f"SELECT p.*, g.name AS group_name FROM projects p LEFT JOIN groups g ON g.id=p.group_id {where} ORDER BY {order}",
                () if args.group is None else (args.group.strip(),))
    rows = cur.fetchall()

    if not rows:
        print("Bento 数据库中暂无项目。")
        return

    print(f"\n📂 Bento 项目列表 (数据库: {DB_PATH}):")
    print("-" * 92)
    print(f"{'ID':<20} | {'类型':<6} | {'端口':<6} | {'显示名称 / 项目名':<22} | {'分组':<12} | {'启动命令'}")
    print("-" * 92)
    for r in rows:
        display = r['display_name'] or r['name']
        port_str = str(r['port']) if r['port'] is not None else "-"
        ptype = r['type'] or 'web'
        group = r['group_name'] or '未分组'
        flags = (' · 置顶' if r['pinned'] else '') + (' · 已归档' if r['archived'] else '')
        command = r['start_command'] if r['runnable'] else '（无需运行）'
        print(f"{r['id']:<20} | {ptype:<6} | {port_str:<6} | {display:<22} | {group:<12} | {command}{flags}")
    print("-" * 92)
    print(f"共 {len(rows)} 个项目已配置。\n")

def cmd_check_port(args):
    conn = get_connection()
    row = port_project(conn, args.port)
    if row:
        display = row['display_name'] or row['name']
        print(f"❌ 端口 {args.port} 已被占用！被项目「{display}」(ID: {row['id']}) 配置。不可重复使用。")
        sys.exit(1)
    else:
        print(f"✅ 端口 {args.port} 可用，未被任何 Bento 项目占用。")

def cmd_add(args):
    if args.no_run and (args.command is not None or args.port is not None):
        print("❌ 错误：使用 --no-run 的项目无需运行，不能同时设置 --command 或 --port。", file=sys.stderr)
        sys.exit(1)
    if not args.no_run and not (args.command or "").strip():
        print("❌ 错误：请用 --command 提供启动命令；无需运行的项目请使用 --no-run。", file=sys.stderr)
        sys.exit(1)
    conn = get_connection()
    conn.execute("BEGIN IMMEDIATE")
    cur = conn.cursor()

    # Hold the write lock across validation and insertion, including older databases.
    ensure_unique_port(conn, args.port)

    # 2. Generate ID if not provided
    project_id = args.id or args.name.lower().replace(" ", "-")

    # Check duplicate ID
    cur.execute("SELECT id FROM projects WHERE id = ?", (project_id,))
    if cur.fetchone():
        suffix = args.port if args.port is not None else "app"
        project_id = f"{project_id}-{suffix}"

    project_type = args.type or "web"

    try:
        with conn:
            columns = ["id", "name", "display_name", "description", "path", "port", "start_command", "type", "runnable"]
            values = [project_id, args.name, args.display or args.name, args.desc or "", args.path, args.port, args.command or "", project_type, 0 if args.no_run else 1]
            if args.group is not None:
                columns.append("group_id")
                values.append(resolve_group(conn, args.group))
            if "sort_order" in project_columns(conn):
                # New projects go to the end of the sidebar, like projects added in the app.
                columns.append("sort_order")
                values.append(conn.execute("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects").fetchone()[0])
            conn.execute(
                f"INSERT INTO projects ({', '.join(columns)}) VALUES ({', '.join('?' * len(columns))})",
                values,
            )
        print(f"✅ 成功添加新项目！")
        print(f"   ID: {project_id}")
        print(f"   名称: {args.name}")
        print(f"   类型: {project_type}")
        print(f"   端口: {args.port if args.port is not None else '无'}")
        print(f"   路径: {args.path}")
        print(f"   启动命令: {'无需运行' if args.no_run else args.command}")
    except sqlite3.IntegrityError as e:
        print(f"❌ 数据库写入失败 (唯一约束冲突): {e}", file=sys.stderr)
        sys.exit(1)

def cmd_update(args):
    conn = get_connection()
    conn.execute("BEGIN IMMEDIATE")
    cur = conn.cursor()

    cur.execute("SELECT * FROM projects WHERE id = ?", (args.id,))
    existing = cur.fetchone()
    if not existing:
        print(f"❌ 未找到 ID 为「{args.id}」的项目！", file=sys.stderr)
        sys.exit(1)

    runnable = bool(existing['runnable']) if args.runnable is None else args.runnable
    if not runnable and (args.command is not None or args.port is not None):
        print("❌ 错误：该项目无需运行，不能设置 --command 或 --port；如需启动请同时使用 --run。", file=sys.stderr)
        sys.exit(1)
    new_name = args.name if args.name is not None else existing['name']
    new_display = args.display if args.display is not None else existing['display_name']
    new_desc = args.desc if args.desc is not None else existing['description']
    new_path = args.path if args.path is not None else existing['path']
    new_port = args.port if args.port is not None else existing['port']
    new_command = args.command if args.command is not None else existing['start_command']
    new_type = args.type if args.type is not None else (existing['type'] if 'type' in existing.keys() else 'web')

    if runnable and not (new_command or "").strip():
        print("❌ 错误：需要运行的项目必须有启动命令，请用 --command 提供。", file=sys.stderr)
        sys.exit(1)

    try:
        services = json.loads(existing['services'])
    except (TypeError, ValueError):
        services = []
    if not runnable:
        new_port, new_command, services = None, "", []
        services_json = "[]"
    elif services:
        primary = next((service for service in services if service.get('openWeb')), None)
        if args.port is not None:
            if not primary:
                print("❌ 错误：请先在 Bento 中选择用于打开网页的服务。", file=sys.stderr)
                sys.exit(1)
            primary['port'] = new_port
        ports = [service['port'] for service in services if service.get('port') is not None]
        if len(ports) != len(set(ports)):
            print("❌ 错误：同一项目的服务端口必须唯一。", file=sys.stderr)
            sys.exit(1)
        for port in ports:
            ensure_unique_port(conn, port, args.id)
        services_json = json.dumps(services, ensure_ascii=False)
    else:
        ensure_unique_port(conn, new_port, args.id)
        services_json = existing['services']

    with conn:
        group_id = resolve_group(conn, args.group) if args.group is not None else existing['group_id']
        conn.execute("""
            UPDATE projects
            SET name = ?, display_name = ?, description = ?, path = ?, port = ?, start_command = ?, type = ?, group_id = ?, services = ?, runnable = ?, updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
        """, (new_name, new_display, new_desc, new_path, new_port, new_command, new_type, group_id,
              services_json, int(runnable), args.id))
        if not runnable and "auto_open" in project_columns(conn):
            conn.execute("UPDATE projects SET auto_open = 0 WHERE id = ?", (args.id,))

    print(f"✅ 成功更新项目「{args.id}」！")
    print(f"   类型: {new_type}, 端口: {new_port}, 路径: {new_path}, 启动命令: {new_command if runnable else '无需运行'}")

def cmd_delete(args):
    conn = get_connection()
    with conn:
        cur = conn.execute("DELETE FROM projects WHERE id = ?", (args.id,))
        if cur.rowcount > 0:
            print(f"✅ 已删除项目「{args.id}」")
        else:
            print(f"⚠️ 未找到项目「{args.id}」", file=sys.stderr)

def main():
    parser = argparse.ArgumentParser(description="Bento SQLite 项目管理 CLI")
    subparsers = parser.add_subparsers(dest="subcommand", required=True)

    # list
    p_list = subparsers.add_parser("list", help="列出所有配置的项目")
    p_list.add_argument("--group", help="仅列出指定分组，空字符串表示未分组")

    # check-port
    p_check = subparsers.add_parser("check-port", help="检查指定端口是否已被占用")
    p_check.add_argument("port", type=int, help="待检测的端口号")

    # add
    p_add = subparsers.add_parser("add", help="新增项目")
    p_add.add_argument("--name", required=True, help="项目名称")
    p_add.add_argument("--path", required=True, help="项目路径（支持 ~）")
    p_add.add_argument("--command", help="启动命令，如 'pnpm dev' 或 './start.sh dev'；使用 --no-run 时省略")
    p_add.add_argument("--no-run", action="store_true", help="无需运行的项目：不填写启动命令和端口，只使用终端、Git 和其他命令")
    p_add.add_argument("--port", type=int, help="项目端口号（若提供则必须唯一）")
    p_add.add_argument("--type", choices=["web", "app"], default="web", help="项目类型：web (前端网页) 或 app (桌面/原生应用，默认 web)")
    p_add.add_argument("--display", help="显示别名")
    p_add.add_argument("--desc", help="项目简要描述")
    p_add.add_argument("--id", help="自定义唯一 ID")
    p_add.add_argument("--group", help="分组名称，不存在则新建；空字符串表示未分组")

    # update
    p_update = subparsers.add_parser("update", help="更新已有项目配置")
    p_update.add_argument("--id", required=True, help="要更新的项目 ID")
    p_update.add_argument("--name", help="新项目名称")
    p_update.add_argument("--display", help="新显示名称")
    p_update.add_argument("--desc", help="新描述")
    p_update.add_argument("--path", help="新路径")
    p_update.add_argument("--port", type=int, help="新端口号（必须唯一）")
    p_update.add_argument("--command", help="新启动命令")
    p_update.add_argument("--type", choices=["web", "app"], help="新项目类型")
    p_update.add_argument("--group", help="分组名称，不存在则新建；空字符串移回未分组，省略则保留")
    run_mode = p_update.add_mutually_exclusive_group()
    run_mode.add_argument("--run", dest="runnable", action="store_const", const=True, help="改为需要运行（需已有或同时提供 --command）")
    run_mode.add_argument("--no-run", dest="runnable", action="store_const", const=False, help="改为无需运行，并清空启动命令、端口和服务")

    # delete
    p_del = subparsers.add_parser("delete", help="删除项目")
    p_del.add_argument("id", help="要删除的项目 ID")

    args = parser.parse_args()

    if args.subcommand == "list":
        cmd_list(args)
    elif args.subcommand == "check-port":
        cmd_check_port(args)
    elif args.subcommand == "add":
        cmd_add(args)
    elif args.subcommand == "update":
        cmd_update(args)
    elif args.subcommand == "delete":
        cmd_delete(args)

if __name__ == "__main__":
    main()
