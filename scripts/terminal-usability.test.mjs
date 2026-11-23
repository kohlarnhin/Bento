import assert from 'node:assert/strict';
import { test } from 'node:test';
import { agentDurationRefreshDelay, agentsByProject, clampTerminalFontSize, createTitlePublisher, firstOpenedTerminalSlot, formatAgentDuration, nextAttentionTarget, normalizeTerminalTitle, projectTerminalSlots, terminalDisplayNumber, terminalSelectionAfterClose, terminalsNeedingAttention } from '../src/lib/terminalUsability.ts';

test('project navigation takes the earliest registered tab and skips placeholders', () => {
  const other = { projectId: 'other', slot: 1, sessionId: 'other' };
  const placeholder = { projectId: 'project', slot: 1, opening: true };
  const third = { projectId: 'project', slot: 3, sessionId: 'third', alive: true };
  const second = { projectId: 'project', slot: 2, sessionId: 'second', alive: true };
  assert.equal(firstOpenedTerminalSlot('project', [other, placeholder]), undefined);
  assert.equal(firstOpenedTerminalSlot('project', [third, other, placeholder, second]), 2);
  assert.equal(firstOpenedTerminalSlot('project', [third, other, placeholder]), 3);
});

test('terminal numbering closes gaps and appends a recycled slot to the end', () => {
  const second = { projectId: 'project', slot: 2, sessionId: 'two', tabOrder: 10 };
  const third = { projectId: 'project', slot: 3, sessionId: 'three', tabOrder: 11 };
  const appended = { projectId: 'project', slot: 1, sessionId: 'new', tabOrder: 12 };
  const remaining = projectTerminalSlots('project', [{ projectId: 'project', slot: 1 }, third, second]);
  assert.deepEqual(remaining, [2, 3]);
  assert.deepEqual([1, 2, 3].map(slot => terminalDisplayNumber(remaining, slot)), [undefined, 1, 2]);
  const reopened = projectTerminalSlots('project', [appended, third, second]);
  assert.deepEqual(reopened, [2, 3, 1]);
  assert.equal(terminalDisplayNumber(reopened, 1), 3);
  assert.equal(firstOpenedTerminalSlot('project', [appended, third, second]), 2);
  assert.deepEqual(projectTerminalSlots('project', []), []);
});

test('closing a selected tab chooses the right neighbor, then the left, then overview', () => {
  const before = [2, 3, 1, 4];
  assert.equal(terminalSelectionAfterClose(2, before, [3, 1, 4]), 3);
  assert.equal(terminalSelectionAfterClose(3, before, [2, 1, 4]), 1);
  assert.equal(terminalSelectionAfterClose(4, before, [2, 3, 1]), 1);
  assert.equal(terminalSelectionAfterClose(3, before, [1, 4]), 1);
  assert.equal(terminalSelectionAfterClose(3, before, [2, 3, 4]), 3);
  assert.equal(terminalSelectionAfterClose(1, [1], []), undefined);
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

test('Attention targets without timestamps have a stable order', () => {
  const targets = [
    { projectId: 'b', slot: 1, attention: true },
    { projectId: 'a', slot: 2, attention: true },
    { projectId: 'a', slot: 1, attention: true },
  ];
  assert.deepEqual(terminalsNeedingAttention(targets).map(t => t.projectId + t.slot), ['a1', 'a2', 'b1']);
});

const agent = (projectId, slot, state, stateStartedAt, acknowledged = false) => ({
  projectId, slot, agent: { kind: 'claude', state, stateStartedAt, roundStartedAt: null, acknowledged },
});

test('Sidebar chooses waiting, then unread completion, then working for each project', () => {
  const thinking = agent('a', 1, 'thinking', 5);
  const done = agent('a', 2, 'done', 10);
  const waiting = agent('a', 3, 'waiting', 20);
  const acknowledged = agent('b', 1, 'done', 15, true);
  let rows = agentsByProject([thinking, done, waiting, acknowledged]);
  assert.deepEqual(rows.get('a'), { agent: waiting.agent, slot: 3, multiple: true });
  assert.equal(rows.has('b'), false);
  rows = agentsByProject([thinking, done]);
  assert.equal(rows.get('a').slot, 2);
  rows = agentsByProject([thinking, { ...done, agent: { ...done.agent, acknowledged: true } }]);
  assert.deepEqual(rows.get('a'), { agent: thinking.agent, slot: 1, multiple: true });
});

test('Cmd+Shift+U visits oldest approvals, unread completions and existing reminders in order', () => {
  const ordinary = { projectId: 'shell', slot: 1, attention: true, attentionAt: 100 };
  const olderOrdinary = { projectId: 'older-shell', slot: 1, attention: true, attentionAt: 90 };
  const later = agent('a', 3, 'waiting', 40);
  const earlier = agent('z', 2, 'waiting', 30);
  const tied = agent('z', 3, 'waiting', 30);
  const done = agent('a', 2, 'done', 5);
  const acknowledged = { ...agent('a', 1, 'done', 3, true), attention: true };
  const thinking = agent('a', 4, 'thinking', 1);
  const targets = [ordinary, done, later, tied, thinking, acknowledged, earlier, olderOrdinary];
  let current;
  for (const expected of [earlier, tied, later, done, ordinary, olderOrdinary, earlier]) {
    current = nextAttentionTarget(targets, current);
    assert.equal(current, expected);
  }
  assert.equal(nextAttentionTarget([earlier], earlier), undefined);
  assert.equal(nextAttentionTarget([acknowledged, thinking]), undefined);
});

test('Agent elapsed time uses seconds, whole minutes, then hours and remaining minutes', () => {
  for (const [elapsed, compact, full] of [
    [-100, '0s', '0 秒'],
    [12_500, '12s', '12 秒'],
    [59_999, '59s', '59 秒'],
    [60_000, '1m', '1 分钟'],
    [270_000, '4m', '4 分钟'],
    [3_599_999, '59m', '59 分钟'],
    [3_600_000, '1h 0m', '1 小时 0 分钟'],
    [3_959_999, '1h 5m', '1 小时 5 分钟'],
  ]) {
    assert.deepEqual(formatAgentDuration(elapsed), { compact, full }, `elapsed ${elapsed} ms`);
  }
});

test('Agent elapsed time refreshes at the next second or minute boundary', () => {
  for (const [elapsed, delay] of [
    [-250, 1_250], [0, 1_000], [12_500, 500], [59_999, 1],
    [60_000, 60_000], [120_500, 59_500], [3_599_999, 1], [3_900_000, 60_000],
  ]) {
    assert.equal(agentDurationRefreshDelay(elapsed), delay, `elapsed ${elapsed} ms`);
  }
});

test('Zoom clamps both bounds under rapid increments/decrements and preserves every allowed size', () => {
  for (let size = 12; size <= 20; size++) assert.equal(clampTerminalFontSize(size), size);
  let size = 14;
  for (let i = 0; i < 30; i++) size = clampTerminalFontSize(size + 1);
  assert.equal(size, 20);
  for (let i = 0; i < 30; i++) size = clampTerminalFontSize(size - 1);
  assert.equal(size, 12);
});
