import assert from 'node:assert/strict';
import { test } from 'node:test';
import { clampTerminalFontSize, createTitlePublisher, firstOpenedTerminalSlot, normalizeTerminalTitle, terminalsNeedingAttention } from '../src/lib/terminalUsability.ts';

test('project navigation takes the earliest opened label, including an exited session', () => {
  const other = { projectId: 'other', slot: 1, sessionId: 'other' };
  const placeholder = { projectId: 'project', slot: 1, opening: true };
  const third = { projectId: 'project', slot: 3, sessionId: 'third', alive: true };
  const second = { projectId: 'project', slot: 2, sessionId: 'second', alive: false };
  assert.equal(firstOpenedTerminalSlot('project', [other, placeholder]), undefined);
  assert.equal(firstOpenedTerminalSlot('project', [third, other, placeholder, second]), 2);
  assert.equal(firstOpenedTerminalSlot('project', [third, other, placeholder]), 3);
});

for (const spinner of ['✳', '✶', '✻', '✽', '·', '✢', '⠋', '⠙', '⠸', '⣿', '🌱', '']) {
  test('Title strips spinner ' + spinner, () => {
    assert.equal(normalizeTerminalTitle(spinner + '  修复登录'), '修复登录');
  });
}

test('Title removes controls, folds whitespace and retains typed letters, digits and CJK', () => {
  assert.equal(normalizeTerminalTitle(' \x00✳\t修复\r\n 登录\x07\u200b  '), '修复 登录');
  assert.equal(normalizeTerminalTitle('... 42 tests → ready'), '42 tests → ready');
  assert.equal(normalizeTerminalTitle('✓ Überprüfung 日本語 한국어'), 'Überprüfung 日本語 한국어');
  assert.equal(normalizeTerminalTitle('✳ ⠋ ·'), '');
});

test('Shell paths are hidden before leading punctuation is removed', () => {
  for (const title of ['~/work', '/private/tmp', 'user@host:~/repo', 'user@host:/Users/me/repo', '✳ /tmp/repo', '· user@host:~/项目']) {
    assert.equal(normalizeTerminalTitle(title), '', title);
  }
  assert.equal(normalizeTerminalTitle('Build /tmp/repo'), 'Build /tmp/repo');
  assert.equal(normalizeTerminalTitle('user@host: build ready'), 'user@host: build ready');
});

test('Title cap counts Unicode characters without splitting surrogate pairs', () => {
  assert.equal(normalizeTerminalTitle('中'.repeat(100)), '中'.repeat(80));
  const raw = 'Task ' + '𠀀'.repeat(100);
  assert.equal([...normalizeTerminalTitle(raw)].length, 80);
  assert.ok(normalizeTerminalTitle(raw).endsWith('𠀀'));
});

test('Title publisher emits at most twice per second and preserves the trailing value', context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const values = [];
  const publisher = createTitlePublisher(title => values.push(title));
  for (let i = 0; i < 21; i++) {
    publisher.update('Task ' + i);
    context.mock.timers.tick(50);
  }
  assert.deepEqual(values, ['Task 9', 'Task 19']);
  context.mock.timers.tick(450);
  assert.deepEqual(values, ['Task 9', 'Task 19', 'Task 20']);
});

test('Spinner churn with identical normalized titles emits once', context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const values = [];
  const publisher = createTitlePublisher(title => values.push(title));
  for (let i = 0; i < 100; i++) {
    publisher.update(['✳', '✶', '⠋', '⠙'][i % 4] + ' 修复登录');
    context.mock.timers.tick(50);
  }
  assert.deepEqual(values, ['修复登录']);
});

test('Returning to the published title cancels pending churn; restart cancels stale trailing work', context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const values = [];
  const publisher = createTitlePublisher(title => values.push(title));
  publisher.update('Ready'); context.mock.timers.tick(500);
  publisher.update('Busy'); context.mock.timers.tick(100);
  publisher.update('Ready'); context.mock.timers.tick(500);
  assert.deepEqual(values, ['Ready']);
  publisher.update('Stale'); publisher.reset(); context.mock.timers.tick(1000);
  assert.deepEqual(values, ['Ready']);
  publisher.update('Ready'); context.mock.timers.tick(500);
  publisher.update('/tmp/path'); context.mock.timers.tick(500);
  assert.deepEqual(values, ['Ready', 'Ready', '']);
});

test('Attention ordering selects the newest, then the next remaining terminal without mutating input', () => {
  const targets = [
    { projectId: 'a', slot: 1, attention: true, attentionAt: 100 },
    { projectId: 'a', slot: 2, attention: true, attentionAt: 300 },
    { projectId: 'b', slot: 1, attention: false, attentionAt: 900 },
    { projectId: 'c', slot: 1, attention: true, attentionAt: 200 },
  ];
  const order = [...targets];
  assert.deepEqual(terminalsNeedingAttention(targets).map(t => t.attentionAt), [300, 200, 100]);
  targets[1].attention = false;
  assert.equal(terminalsNeedingAttention(targets)[0].projectId, 'c');
  assert.deepEqual(targets, order);
  assert.deepEqual(terminalsNeedingAttention([]), []);
});

test('Attention ties and restored timestamps have a stable order', () => {
  const targets = [
    { projectId: 'b', slot: 1, attention: true },
    { projectId: 'a', slot: 2, attention: true },
    { projectId: 'a', slot: 1, attention: true },
  ];
  assert.deepEqual(terminalsNeedingAttention(targets).map(t => t.projectId + t.slot), ['a1', 'a2', 'b1']);
});

test('Zoom clamps both bounds under rapid increments/decrements and preserves every allowed size', () => {
  for (let size = 12; size <= 20; size++) assert.equal(clampTerminalFontSize(size), size);
  let size = 14;
  for (let i = 0; i < 30; i++) size = clampTerminalFontSize(size + 1);
  assert.equal(size, 20);
  for (let i = 0; i < 30; i++) size = clampTerminalFontSize(size - 1);
  assert.equal(size, 12);
});
