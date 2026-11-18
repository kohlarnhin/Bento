import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = fileURLToPath(new URL('./manage_project.py', import.meta.url));
const bootstrap = `
import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location('manage_project', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.DB_PATH = Path(sys.argv[2])
module.OLD_DB_PATH = module.DB_PATH.parent / 'missing-old.db'
sys.argv = [sys.argv[1], *sys.argv[3:]]
module.main()
`;

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'bento-port-cli-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const database = join(directory, 'projects.db');
  const run = (...args) => {
    const result = spawnSync('python3', ['-B', '-c', bootstrap, script, database, ...args], { encoding: 'utf8' });
    assert.ifError(result.error);
    return result;
  };
  const add = (id, port) => run('add', '--id', id, '--name', id, '--display', `项目${id}`, '--path', directory, '--command', 'pnpm dev', ...(port == null ? ['--type', 'app'] : ['--port', String(port)]));
  return { run, add, database };
}

test('CLI add rejects a duplicate port with its project name and id', (t) => {
  const { run, add } = fixture(t);
  assert.equal(add('one', 5173).status, 0);
  const duplicate = add('two', 5173);
  assert.equal(duplicate.status, 1);
  assert.match(duplicate.stderr, /5173.*项目one.*one.*端口必须唯一/);
  assert.doesNotMatch(run('list').stdout, /项目two/);
});

test('CLI update excludes itself and rejects a port assigned to another project', (t) => {
  const { run, add } = fixture(t);
  assert.equal(add('one', 5173).status, 0);
  assert.equal(add('two', 5174).status, 0);
  assert.equal(run('update', '--id', 'one', '--port', '5173').status, 0);
  const duplicate = run('update', '--id', 'two', '--port', '5173');
  assert.equal(duplicate.status, 1);
  assert.match(duplicate.stderr, /项目one.*端口必须唯一/);
  assert.equal(run('update', '--id', 'two', '--port', '5175').status, 0);
});

test('CLI update rechecks unchanged ports in legacy databases without a unique constraint', (t) => {
  const { run, database } = fixture(t);
  const seed = spawnSync('python3', ['-c', `
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as conn:
    conn.execute('CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, display_name TEXT, description TEXT, path TEXT, port INTEGER, start_command TEXT, type TEXT, updated_at TEXT)')
    for project in ['one', 'two']:
        conn.execute('INSERT INTO projects VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', (project, project, project, '', '/tmp', 5173, 'pnpm dev', 'web', ''))
`, database], { encoding: 'utf8' });
  assert.equal(seed.status, 0, seed.stderr);
  const duplicate = run('update', '--id', 'two', '--name', 'changed');
  assert.equal(duplicate.status, 1);
  assert.match(duplicate.stderr, /5173.*one.*端口必须唯一/);
  assert.equal(run('update', '--id', 'two', '--port', '5174').status, 0);
});

test('CLI validates the port range and permits multiple apps without ports', (t) => {
  const { run, add } = fixture(t);
  for (const port of [0, 65536, -1]) {
    const result = add(`invalid-${port}`, port);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /1–65535/);
  }
  assert.equal(add('one', null).status, 0);
  assert.equal(add('two', null).status, 0);
  const result = run('update', '--id', 'one', '--port', '65536');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /1–65535/);
});

test('CLI group add/list/update creates or reuses names and omitted options preserve sidebar metadata', (t) => {
  const { run, database } = fixture(t);
  const add = (id, group) => run('add', '--id', id, '--name', id, '--path', '/tmp', '--command', 'sleep 1', '--type', 'app', '--group', group);
  assert.equal(add('one', '工作').status, 0);
  assert.equal(add('two', '工作').status, 0);
  assert.match(run('list', '--group', '工作').stdout, /one[\s\S]*two/);
  const seed = spawnSync('python3', ['-c', `
import sqlite3,sys
with sqlite3.connect(sys.argv[1]) as conn:
    assert conn.execute('SELECT COUNT(*) FROM groups').fetchone()[0] == 1
    conn.execute("UPDATE projects SET pinned=1,archived=1 WHERE id='one'")
`, database], { encoding: 'utf8' });
  assert.equal(seed.status, 0, seed.stderr);
  assert.equal(run('update', '--id', 'one', '--name', 'changed', '--display', 'changed').status, 0);
  assert.match(run('list', '--group', '工作').stdout, /changed[\s\S]*置顶 · 已归档/);
  assert.equal(run('update', '--id', 'one', '--group', '个人').status, 0);
  assert.doesNotMatch(run('list', '--group', '工作').stdout, /changed/);
  assert.match(run('list', '--group', '个人').stdout, /changed/);
  assert.equal(run('update', '--id', 'one', '--group', '').status, 0);
  assert.match(run('list', '--group', '').stdout, /changed[\s\S]*未分组/);
  assert.doesNotMatch(run('list', '--group', '个人').stdout, /changed/);
});
