import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { nextVersion } from './bump-version.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const versionFiles = ['package.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock'];

for (const [current, next] of [
  ['0.1.0', '0.1.1'], ['0.1.8', '0.1.9'], ['0.1.9', '0.2.0'],
  ['0.9.9', '1.0.0'], ['1.0.9', '1.1.0'], ['1.9.9', '2.0.0'], ['9.9.9', '10.0.0'], ['10.0.0', '10.0.1'],
]) {
  test(`${current} -> ${next}`, () => assert.equal(nextVersion(current), next));
}

test('rejects malformed or out-of-range versions', () => {
  for (const version of ['', 'v0.1.0', '0.1', '0.1.0.0', '0.01.0', '0.1.10', '0.10.0', '00.1.0', '-1.0.0', '0.1.0-beta', '0.1.0\n', ' 0.1.0', null, 100]) {
    assert.throws(() => nextVersion(version), /Invalid version/);
  }
});

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'bento-version-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const name of [...versionFiles, 'scripts/bump-version.mjs']) {
    const destination = join(directory, name);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(join(root, name), destination);
  }
  return directory;
}

function run(directory, ...args) {
  return spawnSync(process.execPath, [join(directory, 'scripts/bump-version.mjs'), ...args], { cwd: directory, encoding: 'utf8' });
}

function contents(directory) {
  return versionFiles.map(name => readFileSync(join(directory, name), 'utf8'));
}

test('--print and --check validate all files without writing', t => {
  const directory = fixture(t);
  const before = contents(directory);
  const version = JSON.parse(before[0]).version;
  const preview = run(directory, '--print');
  assert.equal(preview.status, 0, preview.stderr);
  assert.equal(preview.stdout.trim(), `${version} -> ${nextVersion(version)}`);
  const check = run(directory, '--check');
  assert.equal(check.status, 0, check.stderr);
  assert.match(check.stdout, /all four files/);
  assert.deepEqual(contents(directory), before);
});

test('a bump updates all four files, preserving formatting and other lockfile packages', t => {
  const directory = fixture(t);
  const before = contents(directory);
  const current = JSON.parse(before[0]).version;
  const next = nextVersion(current);
  const result = run(directory);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), `${current} -> ${next}`);
  const after = contents(directory);
  for (let i = 0; i < 3; i += 1) {
    const pattern = i < 2 ? /("version"\s*:\s*")[^"]*(")/ : /(^version\s*=\s*")[^"]*(")/m;
    assert.equal(after[i], before[i].replace(pattern, `$1${next}$2`));
  }
  assert.equal(after[3], before[3].replace(/(\[\[package\]\]\r?\nname = "apphub"\r?\nversion = ")[^"]*(")/, `$1${next}$2`));
  assert.equal(run(directory, '--check').status, 0);
  assert.deepEqual(readdirSync(join(directory, 'src-tauri')).sort(), ['Cargo.lock', 'Cargo.toml', 'tauri.conf.json']);
});

test('mismatched versions fail before changing any file', t => {
  const directory = fixture(t);
  const path = join(directory, 'src-tauri/tauri.conf.json');
  writeFileSync(path, readFileSync(path, 'utf8').replace(/("version"\s*:\s*")[^"]*"/, '$19.9.8"'));
  const before = contents(directory);
  for (const args of [[], ['--check'], ['--print']]) {
    const result = run(directory, ...args);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Version mismatch/);
    assert.deepEqual(contents(directory), before);
  }
});

test('a malformed final file fails before changing the preceding three files', t => {
  const directory = fixture(t);
  const path = join(directory, 'src-tauri/Cargo.lock');
  writeFileSync(path, readFileSync(path, 'utf8').replace(/(name = "apphub"\r?\nversion = ")[^"]*"/, '$10.1.10"'));
  const before = contents(directory);
  const result = run(directory);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Invalid version/);
  assert.deepEqual(contents(directory), before);
});

test('missing or duplicate apphub lockfile entries fail before writing', t => {
  const directory = fixture(t);
  const path = join(directory, 'src-tauri/Cargo.lock');
  const original = readFileSync(path, 'utf8');
  const version = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8')).version;
  for (const text of [original.replace('name = "apphub"', 'name = "other-app"'), `${original}\n[[package]]\nname = "apphub"\nversion = "${version}"\n`]) {
    writeFileSync(path, text);
    const before = contents(directory);
    const result = run(directory);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /expected exactly one apphub package/);
    assert.deepEqual(contents(directory), before);
  }
});

test('a filesystem error during replacement rolls back every version file', t => {
  const directory = fixture(t);
  const before = contents(directory);
  const fault = `
    import fs from 'node:fs';
    import { syncBuiltinESMExports } from 'node:module';
    const rename = fs.renameSync;
    fs.renameSync = (source, destination) => {
      if (source.endsWith('.tmp') && destination.endsWith('/Cargo.toml')) throw new Error('Simulated filesystem failure');
      return rename(source, destination);
    };
    syncBuiltinESMExports();
  `;
  const result = spawnSync(process.execPath, ['--import', `data:text/javascript,${encodeURIComponent(fault)}`, join(directory, 'scripts/bump-version.mjs')], { cwd: directory, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Simulated filesystem failure/);
  assert.deepEqual(contents(directory), before);
  assert.deepEqual(readdirSync(directory).sort(), ['package.json', 'scripts', 'src-tauri']);
  assert.deepEqual(readdirSync(join(directory, 'src-tauri')).sort(), ['Cargo.lock', 'Cargo.toml', 'tauri.conf.json']);
});

test('unknown arguments fail without writing', t => {
  const directory = fixture(t);
  const before = contents(directory);
  const result = run(directory, '--unknown');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Usage:/);
  assert.deepEqual(contents(directory), before);
});
