import assert from 'node:assert/strict';
import { test } from 'node:test';
import { defaultServiceKey, injectedPortVariables, projectPorts, sameServiceStatuses, serviceEnvName } from '../src/lib/projectServices.ts';
import { applyInspectionDiff, diffProjectInspection } from '../src/lib/projectRescan.ts';

const frontend = {
  id: 'service:frontend', name: '前端', dir: 'frontend', port: 5173, kind: 'web', key: 'WEB', openWeb: true,
  portCandidate: { source: 'package.json', port: 5173, confidence: 'high' },
};
const backend = {
  id: 'service:backend', name: '后端', dir: 'backend', port: 8000, kind: 'api', key: 'API', openWeb: false,
  portCandidate: { source: '.env', port: 8000, confidence: 'high' },
};
const inspection = {
  commands: ['npm start'], port: 5173, portSource: 'package.json', projectType: 'web', portCandidates: [],
  services: [frontend, backend], startScripts: [{ command: './dev.sh', source: 'dev.sh' }],
};

test('Rescan suggests the root launcher and both services without changing the current form', () => {
  const current = { startCommand: 'npm start', type: 'web', port: '6000', services: [] };
  const diff = diffProjectInspection(current, inspection);
  assert.deepEqual(diff.serviceChanges.map(({ service, added }) => [service.dir, added]), [['frontend', true], ['backend', true]]);
  assert.deepEqual(diff.services.map(({ port, openWeb }) => [port, openWeb]), [['5173', true], ['8000', false]]);
  assert.deepEqual(diff.startScripts, inspection.startScripts);
  assert.deepEqual(applyInspectionDiff(diff.rows, new Set(['startCommand'])), { startCommand: './dev.sh' });
  assert.equal(current.services.length, 0);
  assert.equal(current.port, '6000');
});

test('Service rescan preserves configured IDs, naming and default when detecting changed ports', () => {
  const services = [
    { ...frontend, id: 'existing-frontend', name: '网站', dir: 'frontend/', port: '5172', key: 'SITE', openWeb: false },
    { ...backend, id: 'existing-backend', port: '8000', openWeb: true },
    { dir: 'worker', id: 'worker', name: '计算', port: '', openWeb: false },
  ];
  const diff = diffProjectInspection({ startCommand: './dev.sh', type: 'web', port: '8000', services }, inspection);
  assert.equal(diff.serviceChanges.length, 1);
  assert.equal(diff.serviceChanges[0].previousPort, '5172');
  assert.deepEqual(diff.services.map(({ name, id, port, openWeb }) => [name, id, port, openWeb]), [
    ['网站', 'existing-frontend', '5173', false], ['后端', 'existing-backend', '8000', true], ['计算', 'worker', '', false],
  ]);
  assert.equal(services[0].port, '5172');
  assert.equal(diff.services[0].key, 'SITE');
});

test('Port preview uses role keys and omits explicit overrides and unset ports', () => {
  const project = {
    services: [frontend, backend, { ...backend, dir: 'apps/my.worker/', port: null }],
    port: 5173, env: [{ key: 'PORT', value: '42' }, { key: 'BENTO_PORT_API', value: '' }], injectPorts: true,
  };
  assert.equal(serviceEnvName(defaultServiceKey(undefined, 'apps/my.worker/')), 'BENTO_PORT_MY_WORKER');
  assert.equal(serviceEnvName(defaultServiceKey(undefined, 'services/前端K')), 'BENTO_PORT____');
  assert.equal(defaultServiceKey('web', 'apps/任何目录'), 'WEB');
  assert.equal(defaultServiceKey('api', 'services/example'), 'API');
  assert.deepEqual(projectPorts(project), [5173, 8000]);
  assert.deepEqual(injectedPortVariables(project), [
    { key: 'BENTO_PORT_WEB', value: '5173' }, { key: 'BENTO_PORTS', value: 'WEB:5173,API:8000' },
  ]);
  assert.deepEqual(injectedPortVariables({ ...project, injectPorts: false }), []);
  assert.deepEqual(injectedPortVariables({ services: [], port: 5173, env: [], injectPorts: true }), [
    { key: 'PORT', value: '5173' },
  ]);
  assert.deepEqual(injectedPortVariables({ services: [frontend], port: 5173, env: [], injectPorts: true }), [
    { key: 'PORT', value: '5173' },
  ]);
});

test('Rescan gives new services unique keys and keeps the configured standard launcher', () => {
  const extra = { ...frontend, id: 'service:admin', name: '后台', dir: 'admin', port: 5180, openWeb: false };
  const first = { command: './scripts/bento-dev.sh', source: 'scripts/bento-dev.sh' };
  const saved = { command: './scripts/bento-dev-2.sh', source: 'scripts/bento-dev-2.sh' };
  const diff = diffProjectInspection({ type: 'web', port: '5173', startCommand: saved.command,
    services: [{ ...frontend, port: '5173' }, { ...backend, port: '8000' }] },
  { ...inspection, services: [frontend, backend, extra], startScripts: [first, saved] });
  assert.equal(diff.services[2].key, 'WEB_2');
  assert.equal(diff.rows.find(row => row.field === 'startCommand').changed, false);
});

test('Runtime snapshots compare listener values rather than the deserialized array identity', () => {
  const statuses = [{ serviceId: 'frontend', port: 5173, listening: true, owner: { pid: 2, inProject: true } }];
  assert.equal(sameServiceStatuses(statuses, structuredClone(statuses)), true);
  assert.equal(sameServiceStatuses(statuses, [{ ...statuses[0], owner: { pid: 2, inProject: false } }]), false);
  assert.equal(sameServiceStatuses([], statuses), false);
});
