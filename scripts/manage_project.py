#!/usr/bin/env python3
"""
Bento SQLite Database Project Manager CLI for AI and Developers.

Database: ~/.apphub/apphub.db
Usage:
    python3 manage_project.py list
    python3 manage_project.py add --name <name> --path <path> --command <command> [--port <port>] [--type <web|app>] [--display <display>] [--desc <desc>]
    python3 manage_project.py update --id <id> [--name <name>] [--path <path>] [--port <port>] [--command <command>] [--type <web|app>]
    python3 manage_project.py delete <id>
    python3 manage_project.py check-port <port>
"""

import sys
import os
import sqlite3
import argparse
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
        # sort_order / env / commands / auto_open are migrated by the Bento app itself,
        # which also preserves the existing sidebar order. An empty database stays empty.

def project_columns(conn):
    return {c[1] for c in conn.execute("PRAGMA table_info(projects)").fetchall()}

def cmd_list(args):
    conn = get_connection()
    cur = conn.cursor()
    # Same order as the Bento sidebar; databases the app hasn't migrated yet keep the old port order.
    order = "sort_order IS NULL, sort_order, created_at, id" if "sort_order" in project_columns(conn) else "port ASC"
    cur.execute(f"SELECT id, name, display_name, type, port, path, start_command FROM projects ORDER BY {order}")
    rows = cur.fetchall()

    if not rows:
        print("Bento 数据库中暂无项目。")
        return

    print(f"\n📂 Bento 项目列表 (数据库: {DB_PATH}):")
    print("-" * 92)
    print(f"{'ID':<20} | {'类型':<6} | {'端口':<6} | {'显示名称 / 项目名':<22} | {'启动命令'}")
    print("-" * 92)
    for r in rows:
        display = r['display_name'] or r['name']
        port_str = str(r['port']) if r['port'] is not None else "-"
        ptype = r['type'] or 'web'
        print(f"{r['id']:<20} | {ptype:<6} | {port_str:<6} | {display:<22} | {r['start_command']}")
    print("-" * 92)
    print(f"共 {len(rows)} 个项目已配置。\n")

def cmd_check_port(args):
    conn = get_connection()
    cur = conn.cursor()
    cur.execute("SELECT id, name, display_name FROM projects WHERE port = ?", (args.port,))
    row = cur.fetchone()
    if row:
        display = row['display_name'] or row['name']
        print(f"❌ 端口 {args.port} 已被占用！被项目「{display}」(ID: {row['id']}) 配置。不可重复使用。")
        sys.exit(1)
    else:
        print(f"✅ 端口 {args.port} 可用，未被任何 Bento 项目占用。")

def cmd_add(args):
    conn = get_connection()
    cur = conn.cursor()

    # 1. Check duplicate port if port is specified
    if args.port is not None:
        cur.execute("SELECT id, name, display_name FROM projects WHERE port = ?", (args.port,))
        conflict = cur.fetchone()
        if conflict:
            display = conflict['display_name'] or conflict['name']
            print(f"❌ 错误：端口 {args.port} 已被项目「{display}」(ID: {conflict['id']}) 占用！", file=sys.stderr)
            print(f"👉 提示：Bento 要求各项目端口必须唯一，请为该项目分配其他空闲端口（例如 {args.port + 1}）后再试。", file=sys.stderr)
            sys.exit(1)

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
            columns = ["id", "name", "display_name", "description", "path", "port", "start_command", "type"]
            values = [project_id, args.name, args.display or args.name, args.desc or "", args.path, args.port, args.command, project_type]
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
        print(f"   启动命令: {args.command}")
    except sqlite3.IntegrityError as e:
        print(f"❌ 数据库写入失败 (唯一约束冲突): {e}", file=sys.stderr)
        sys.exit(1)

def cmd_update(args):
    conn = get_connection()
    cur = conn.cursor()

    cur.execute("SELECT * FROM projects WHERE id = ?", (args.id,))
    existing = cur.fetchone()
    if not existing:
        print(f"❌ 未找到 ID 为「{args.id}」的项目！", file=sys.stderr)
        sys.exit(1)

    new_name = args.name if args.name is not None else existing['name']
    new_display = args.display if args.display is not None else existing['display_name']
    new_desc = args.desc if args.desc is not None else existing['description']
    new_path = args.path if args.path is not None else existing['path']
    new_port = args.port if args.port is not None else existing['port']
    new_command = args.command if args.command is not None else existing['start_command']
    new_type = args.type if args.type is not None else (existing['type'] if 'type' in existing.keys() else 'web')

    # Check port uniqueness with other projects
    if new_port is not None and new_port != existing['port']:
        cur.execute("SELECT id, name, display_name FROM projects WHERE port = ? AND id != ?", (new_port, args.id))
        conflict = cur.fetchone()
        if conflict:
            display = conflict['display_name'] or conflict['name']
            print(f"❌ 错误：端口 {new_port} 已被其他项目「{display}」占用！请更换其他端口。", file=sys.stderr)
            sys.exit(1)

    with conn:
        conn.execute("""
            UPDATE projects
            SET name = ?, display_name = ?, description = ?, path = ?, port = ?, start_command = ?, type = ?, updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
        """, (new_name, new_display, new_desc, new_path, new_port, new_command, new_type, args.id))

    print(f"✅ 成功更新项目「{args.id}」！")
    print(f"   类型: {new_type}, 端口: {new_port}, 路径: {new_path}, 启动命令: {new_command}")

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
    subparsers.add_parser("list", help="列出所有配置的项目")

    # check-port
    p_check = subparsers.add_parser("check-port", help="检查指定端口是否已被占用")
    p_check.add_argument("port", type=int, help="待检测的端口号")

    # add
    p_add = subparsers.add_parser("add", help="新增项目")
    p_add.add_argument("--name", required=True, help="项目名称")
    p_add.add_argument("--path", required=True, help="项目路径（支持 ~）")
    p_add.add_argument("--command", required=True, help="启动命令，如 'pnpm dev' 或 './start.sh dev'")
    p_add.add_argument("--port", type=int, help="项目端口号（若提供则必须唯一）")
    p_add.add_argument("--type", choices=["web", "app"], default="web", help="项目类型：web (前端网页) 或 app (桌面/原生应用，默认 web)")
    p_add.add_argument("--display", help="显示别名")
    p_add.add_argument("--desc", help="项目简要描述")
    p_add.add_argument("--id", help="自定义唯一 ID")

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
