import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applyInspectionDiff, diffProjectInspection } from '../src/lib/projectRescan.ts';

const current = { startCommand: 'npm run dev', port: '5200', type: 'web', name: '项目', autoOpen: true };
const inspection = {
  path: '/fixture', exists: true, isDir: true, name: '扫描到的名称', projectType: 'web',
  packageManager: 'pnpm', commands: ['pnpm dev', 'pnpm start'], port: 5210, portSource: '.env',
  portCandidates: [{ port: 5210, source: '.env、vite.config.ts', confidence: 'high' }], existingProject: null,
};

test('A rescan compares the current command, port and type and retains actual port provenance', () => {
  const { rows, sourceFiles } = diffProjectInspection(current, inspection);
  assert.deepEqual(rows.map(({ field, before, after, changed }) => [field, before, after, changed]), [
    ['startCommand', 'npm run dev', 'pnpm dev', true], ['port', '5200', '5210', true], ['type', 'web', 'web', false],
  ]);
  assert.deepEqual(sourceFiles, ['.env', 'vite.config.ts']);
  assert.equal(rows[1].source, '.env');
  assert.equal(rows[1].confidence, 'high');
});

test('Formatting-only differences do not produce changes', () => {
  const { rows } = diffProjectInspection({ ...current, startCommand: ' pnpm dev ', port: '05210' }, inspection);
  assert.ok(rows.every(row => !row.changed));
  assert.deepEqual(applyInspectionDiff(rows, new Set(['startCommand', 'port', 'type'])), {});
});

test('An unknown command or port does not erase configured values', () => {
  const { rows, sourceFiles } = diffProjectInspection(current, { ...inspection, commands: [], port: null, portSource: null, portCandidates: [] });
  assert.ok(rows.every(row => !row.changed));
  assert.deepEqual(rows.map(row => row.after), [current.startCommand, current.port, current.type]);
  assert.deepEqual(sourceFiles, []);
});

test('A recommendation is attributed as a recommendation, not as a source file', () => {
  const { rows, sourceFiles } = diffProjectInspection(current, { ...inspection, portSource: '推荐空闲端口', portCandidates: [] });
  assert.equal(rows[1].source, '推荐空闲端口');
  assert.deepEqual(sourceFiles, []);
});

test('Applying a checked subset gives only the chosen changed fields without mutating the current values', () => {
  const { rows } = diffProjectInspection(current, inspection);
  const patch = applyInspectionDiff(rows, new Set(['port', 'type']));
  assert.deepEqual(patch, { port: '5210' });
  assert.deepEqual({ ...current, ...patch }, { ...current, port: '5210' });
  assert.equal(current.port, '5200');
  assert.deepEqual(applyInspectionDiff(rows, new Set()), {});
  const desktop = diffProjectInspection(current, { ...inspection, projectType: 'app', commands: ['pnpm tauri dev'] });
  assert.deepEqual(applyInspectionDiff(desktop.rows, new Set(['type'])), { type: 'app' });
});
