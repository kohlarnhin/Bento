import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  cleanTerminalText, cpuChart, dashboardLog, dashboardPortListening, dashboardServices, dashboardStatus, durationText, logSeverity,
  recentLogCounts, relativeTime, runInterval, DASHBOARD_LOG_LIMIT,
} from '../src/lib/dashboard.ts';

const front = { id: 'front', name: '前端', key: 'WEB', dir: 'apps/fixture-frontend/', kind: 'web', port: 3310, openWeb: true };
const api = { id: 'api', name: '后端 API', key: 'API', dir: 'services/server', kind: 'api', port: 8310, openWeb: false };

test('dashboard keeps multi-service order and displays legacy and explicit single services with PORT', () => {
  assert.deepEqual(dashboardServices({ services: [front, api], port: 3310 }), [front, api]);
  assert.deepEqual(dashboardServices({ services: [front], port: 3310 }), [{ ...front, key: 'PORT', name: '服务' }]);
  assert.deepEqual(dashboardServices({ services: [], port: null }), [
    { id: 'legacy:root', name: '服务', dir: '.', key: 'PORT', port: null, openWeb: false },
  ]);
});

test('the overview does not display an unrelated single-service listener as a running project', () => {
  assert.equal(dashboardStatus('running', { external: true }, { external: false }), 'stopped');
  assert.equal(dashboardStatus('stopped', { external: false }, { external: true }), 'running');
  assert.equal(dashboardStatus('starting', { isManaged: true }, { external: false }), 'starting');
});

test('open web actions depend only on the same configured port listening, including an unidentified owner', () => {
  assert.equal(dashboardPortListening(front, [{ ...front, state: 'listening', health: { code: 503 } }]), true);
  assert.equal(dashboardPortListening(front, [{ ...front, state: 'occupied', health: { code: null } }]), true);
  assert.equal(dashboardPortListening(front, [{ ...front, state: 'idle' }]), false);
  assert.equal(dashboardPortListening(front, [{ ...front, port: 9999, state: 'listening' }]), false);
  assert.equal(dashboardPortListening(front, [{ ...api, port: front.port, state: 'listening' }]), false);
  assert.equal(dashboardPortListening({ ...front, port: null }, [{ ...front, port: null, state: 'listening' }]), false);
});
test('startup log tags match names, role keys, and service directories while stripping terminal controls', () => {
  const entry = { id: '1', projectId: 'p', stream: 'stdout', timestamp: 0 };
  const parse = text => dashboardLog({ ...entry, text }, [front, api]);
  assert.equal(parse('\x1b[31m[API] ERROR\x1b[0m not ready').text, 'ERROR not ready');
  assert.equal(parse('[前端] ready').service.id, 'front');
  assert.equal(parse('fixture-frontend | ready').service.id, 'front');
  assert.equal(parse('services/server|GET /api/booking 503').severity, 'error');
  assert.equal(parse('后端 API| WARNING restart').severity, 'warning');
  assert.equal(parse('[vite] hmr update').service, undefined);
  assert.equal(parse('[vite] hmr update').text, '[vite] hmr update');
  assert.equal(cleanTerminalText('link \x1b]8;;https://local\x1b\\value\x1b]8;;\x1b\\\x07'), 'link value');
  assert.equal(DASHBOARD_LOG_LIMIT, 500);
});

test('only actual problems appear in the counts for the latest 15 minutes', () => {
  const now = 900_003;
  const entries = ['UnexpectedException', 'WARN wrapper', 'pid 500', 'HTTP/1.1 500 Internal Error',
    'GET /picture/503 200', 'request failed', 'Traceback'];
  const lines = entries.map((text, index) => ({ timestamp: index + 1, severity: logSeverity(text) }));
  assert.deepEqual(recentLogCounts(lines, now), { errors: 3, warnings: 0 });
});

test('run labels use minute and hour durations and remain valid across midnight', () => {
  assert.equal(durationText(60_001), '1 分钟');
  assert.equal(durationText(61 * 60_000), '1 小时 1 分钟');
  assert.equal(relativeTime(0, 125 * 60_000), '2 小时前');
  const started = new Date(2026, 10, 18, 23, 58).getTime();
  const stopped = new Date(2026, 10, 19, 0, 1).getTime();
  assert.match(runInterval(started, stopped, stopped), /^11月18日 23:58 – 今天 00:01$/u);
});

test('CPU charts handle an empty series, a single sample and multiple endpoints', () => {
  assert.equal(cpuChart([], 300, 26), '');
  assert.equal(cpuChart([{ at: 120, value: 0.1 }], 300, 26), '0.00,21.82 300.00,21.82');
  assert.equal(cpuChart([{ at: 120, value: 10 }, { at: 5120, value: 20 }], 300, 26), '0.00,13.00 300.00,4.00');
});
