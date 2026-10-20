---
name: bento
description: >-
  Manage local projects (frontend Web and desktop App) in Bento's (formerly AppHub) SQLite database (~/.apphub/apphub.db).
  Use this skill when the user asks to add, configure, list, update, remove projects,
  assign ports, or manage Web/App applications in Bento.
---

# Bento Project Manager Skill

Bento (formerly AppHub) is a macOS desktop application that aggregates, manages, and launches local projects (including Web frontend and desktop native apps). All project configurations are centrally managed in a local SQLite database at `~/.apphub/apphub.db`.

---

## 🎯 Key Rules for AI

1. **Project Types**:
   - `web`: Frontend web projects that run dev servers with an HTTP port. These display the port and "Open in browser" buttons.
   - `app`: Desktop / native applications (like Tauri apps, Electron apps, etc.). These do NOT show "Open in browser" buttons.
2. **Port Uniqueness**:
   - For projects specifying a port, the port **MUST** be unique across all configured projects.
   - If a port is already taken, do **NOT** assign that port to another project. Suggest or assign an available port.
3. **No Need to Edit App Source Code**:
   - Do **NOT** modify Bento source code to add projects.
   - Prefer the Python CLI below; it keeps the sidebar order and port rules. Users can also add, edit and delete projects in the Bento UI (sidebar「+」/ Cmd+N).
   - After changing the database, ask the user to click the refresh button in Bento's sidebar header to reload the project list.

---

## 🛠️ Management via Helper Script

The helper script is located at:
`scripts/manage_project.py` (or `/Users/zhujiangyong/Software/apphub/scripts/manage_project.py`)

### 1. List Projects & Check Allocated Ports
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py list
```

### 2. Check if a Specific Port is Free
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py check-port <port>
```

### 3. Add a New Project
- **Web Frontend**:
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py add \
  --name "<project-name>" \
  --display "<display-name>" \
  --desc "<description>" \
  --path "<path-or-tilde-path>" \
  --port <unique-port> \
  --command "<start-command>" \
  --type web
```

- **Desktop / Native App**:
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py add \
  --name "<project-name>" \
  --display "<display-name>" \
  --desc "<description>" \
  --path "<path-or-tilde-path>" \
  --command "<start-command>" \
  --port <port-if-any> \
  --type app
```

### 4. Update an Existing Project
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py update \
  --id "<project-id>" \
  --port <new-port> \
  --command "<new-command>" \
  --type <web|app>
```

### 5. Remove a Project
```bash
python3 /Users/zhujiangyong/Software/apphub/scripts/manage_project.py delete "<project-id>"
```

---

## 🗄️ Direct SQLite Access

If executing raw SQL via `sqlite3 ~/.apphub/apphub.db`:

```sql
-- View all projects in sidebar order
SELECT id, name, display_name, type, port, path, start_command FROM projects
ORDER BY sort_order IS NULL, sort_order, created_at, id;

-- Insert a web project at the end of the sidebar
INSERT INTO projects (id, name, display_name, description, path, port, start_command, type, sort_order)
VALUES ('my-web', 'my-web', '我的网页', '描述', '~/Projects/my-web', 3000, 'pnpm dev', 'web',
        (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects));

-- Delete project
DELETE FROM projects WHERE id = 'my-web';
```

Other columns: `env` (JSON array of `{"key","value"}`), `commands` (JSON array of `{"name","command"}`) and `auto_open` (0/1) hold the per-project launch settings; edit them in the Bento UI (编辑 → 高级设置) rather than by hand.
