import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { prepareRelease, releaseNotes } from './release-manifest.mjs';

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'bento-release-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const release = join(directory, 'release');
  mkdirSync(release);
  for (const arch of ['arm64', 'x64']) {
    for (const suffix of ['.dmg', '.app.tar.gz', '.app.tar.gz.sig']) {
      writeFileSync(join(release, `Bento_0.1.0_${arch}${suffix}`), suffix.endsWith('.sig') ? `signature-${arch}\n` : 'artifact');
    }
  }
  return { directory: release, version: '0.1.0', repository: 'kohlarnhin/Bento', notes: 'Bento 首个版本' };
}

test('manifest contains both updater platforms, signatures, URLs and release notes', t => {
  const options = fixture(t);
  const manifest = prepareRelease(options);
  assert.deepEqual(JSON.parse(readFileSync(join(options.directory, 'latest.json'), 'utf8')), manifest);
  assert.equal(manifest.version, '0.1.0');
  assert.equal(manifest.notes, 'Bento 首个版本');
  assert.ok(Number.isFinite(Date.parse(manifest.pub_date)));
  assert.deepEqual(manifest.platforms, {
    'darwin-aarch64': { signature: 'signature-arm64', url: 'https://github.com/kohlarnhin/Bento/releases/download/v0.1.0/Bento_0.1.0_arm64.app.tar.gz' },
    'darwin-x86_64': { signature: 'signature-x64', url: 'https://github.com/kohlarnhin/Bento/releases/download/v0.1.0/Bento_0.1.0_x64.app.tar.gz' },
  });
  assert.equal(readFileSync(join(options.directory, '..', 'release-notes.md'), 'utf8'), 'Bento 首个版本\n');
});

for (const arch of ['arm64', 'x64']) {
  for (const suffix of ['.dmg', '.app.tar.gz', '.app.tar.gz.sig']) {
    for (const failure of ['missing', 'empty']) {
      test(`rejects ${failure} ${arch}${suffix} before writing outputs`, t => {
        const options = fixture(t);
        const path = join(options.directory, `Bento_0.1.0_${arch}${suffix}`);
        if (failure === 'missing') rmSync(path);
        else writeFileSync(path, '');
        assert.throws(() => prepareRelease(options), new RegExp(`${failure === 'missing' ? 'Missing' : 'Empty'}.*Bento_0.1.0_${arch}`));
        assert.equal(existsSync(join(options.directory, 'latest.json')), false);
        assert.equal(existsSync(join(options.directory, '..', 'release-notes.md')), false);
      });
    }
  }
}

test('rejects whitespace-only signatures', t => {
  const options = fixture(t);
  writeFileSync(join(options.directory, 'Bento_0.1.0_x64.app.tar.gz.sig'), ' \n\t');
  assert.throws(() => prepareRelease(options), /Empty updater signature/);
  assert.equal(existsSync(join(options.directory, 'latest.json')), false);
});

test('rejects malformed versions and repository names', t => {
  const options = fixture(t);
  assert.throws(() => prepareRelease({ ...options, version: '0.1.10' }), /Invalid version/);
  assert.throws(() => prepareRelease({ ...options, repository: 'https://github.com/kohlarnhin/Bento' }), /Invalid repository/);
});

test('notes handle the first release and exclude release chores using a read-only git command fixture', t => {
  const options = fixture(t);
  const bin = join(options.directory, 'bin');
  mkdirSync(bin);
  const commands = join(bin, 'commands.jsonl');
  const responses = join(bin, 'responses.json');
  writeFileSync(join(bin, 'git'), `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
fs.appendFileSync(path.join(__dirname, 'commands.jsonl'), JSON.stringify(args) + '\\n');
const responses = JSON.parse(fs.readFileSync(path.join(__dirname, 'responses.json'), 'utf8'));
if (!Object.hasOwn(responses, args[0])) process.exit(2);
process.stdout.write(responses[args[0]]);
`, { mode: 0o755 });
  const previousPath = process.env.PATH;
  process.env.PATH = `${bin}:${previousPath}`;
  t.after(() => { process.env.PATH = previousPath; });
  writeFileSync(responses, JSON.stringify({ tag: '' }));
  assert.equal(releaseNotes(options.directory), 'Bento 首个版本');
  writeFileSync(responses, JSON.stringify({ tag: 'v0.1.0\n', describe: 'v0.1.0\n', log: 'feat: 新增项目\nchore(release): 0.1.1\nfix: 启动状态\n' }));
  assert.equal(releaseNotes(options.directory), '新功能\n- 新增项目\n\n修复\n- 启动状态');
  assert.deepEqual(readFileSync(commands, 'utf8').trim().split('\n').map(line => JSON.parse(line)), [
    ['tag', '--merged', 'HEAD', '--list', 'v*'],
    ['tag', '--merged', 'HEAD', '--list', 'v*'],
    ['describe', '--tags', '--match', 'v*', '--abbrev=0', 'HEAD'],
    ['log', '--no-merges', '--format=%s', 'v0.1.0..HEAD'],
  ]);
});

function fakeGit(t, directory, responses) {
  const bin = join(directory, 'bin');
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, 'responses.json'), JSON.stringify(responses));
  writeFileSync(join(bin, 'git'), `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const responses = JSON.parse(fs.readFileSync(path.join(__dirname, 'responses.json'), 'utf8'));
if (!Object.hasOwn(responses, args[0])) process.exit(2);
process.stdout.write(responses[args[0]]);
`, { mode: 0o755 });
  const previousPath = process.env.PATH;
  process.env.PATH = `${bin}:${previousPath}`;
  t.after(() => { process.env.PATH = previousPath; });
}

test('generated notes keep only user-facing commits, grouped, without type prefixes', t => {
  const options = fixture(t);
  fakeGit(t, options.directory, { tag: 'v0.1.0\n', describe: 'v0.1.0\n', log: [
    'test: debug hook', 'feat(ui): settings dialog', 'docs: skill', 'fix!: crash on quit',
    'style: softer focus halo', 'chore(release): v0.1.1', 'refactor: split module', 'perf: faster logs', 'Merge-like subject without type',
    'FEAT: upper-case type', 'Fix(ui)!: mixed-case type',
  ].join('\n') + '\n' });
  assert.equal(releaseNotes(options.directory),
    '新功能\n- settings dialog\n- upper-case type\n\n修复\n- crash on quit\n- mixed-case type\n\n改进\n- softer focus halo\n- faster logs');
});

test('generated notes fall back to a generic line when nothing is user-facing', t => {
  const options = fixture(t);
  fakeGit(t, options.directory, { tag: 'v0.1.0\n', describe: 'v0.1.0\n', log: 'test: a\ndocs: b\nchore(release): v0.1.1\n' });
  assert.equal(releaseNotes(options.directory), '本次更新包含内部改进。');
});

test('hand-written release-notes/v<version>.md wins, and a blank file falls back', t => {
  const options = fixture(t);
  fakeGit(t, options.directory, { tag: 'v0.1.0\n', describe: 'v0.1.0\n', log: 'feat: from commits\n' });
  mkdirSync(join(options.directory, 'release-notes'));
  writeFileSync(join(options.directory, 'release-notes', 'v0.1.1.md'), '\n修复\n- 手写说明\n\n');
  assert.equal(releaseNotes(options.directory, '0.1.1'), '修复\n- 手写说明');
  assert.equal(releaseNotes(options.directory, '0.1.2'), '新功能\n- from commits');
  writeFileSync(join(options.directory, 'release-notes', 'v0.1.1.md'), '  \n');
  assert.equal(releaseNotes(options.directory, '0.1.1'), '新功能\n- from commits');
});
