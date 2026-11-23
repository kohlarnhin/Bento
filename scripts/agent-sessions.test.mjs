import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { test } from 'node:test';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';

const source = new URL('../src/', import.meta.url).href;
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL?.startsWith(source) && specifier.startsWith('.')) {
      for (const extension of ['.ts', '.tsx']) {
        const path = new URL(specifier + extension, context.parentURL);
        if (existsSync(path)) return next(path.href, context);
      }
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith(source) && url.endsWith('.tsx')) {
      const result = ts.transpileModule(readFileSync(new URL(url), 'utf8'), {
        fileName: url,
        compilerOptions: { module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
      });
      return { shortCircuit: true, format: 'module', source: result.outputText };
    }
    return next(url, context);
  },
});
const { AgentSessionMenuItems, AgentRecoveryCard } = await import('../src/components/AgentSessionActions.tsx');
const { activateAgentTerminal, agentSessionDate, loadAgentIndex } = await import('../src/lib/agentSessions.ts');
const { quickSwitcherResultKey, quickSwitcherSessionOptions } = await import('../src/lib/quickSwitcher.ts');
hooks.deregister();

const claude = {
  id: '383c4fef-fd6e-452a-9130-bdbdea62bc01', tool: 'claude', cwd: '/local/project',
  title: '筛选条件进 URL', startedAt: new Date(2026, 10, 20, 22, 52).getTime(),
  lastAt: new Date(2026, 10, 20, 23, 40).getTime(), durationMs: 48 * 60_000, fileCount: 4,
};
const codex = { ...claude, id: '876c310b-5293-4557-ae09-11c6cb346802', tool: 'codex', title: '导出 CSV 加时间范围', fileCount: null };

function buttons(element) {
  if (Array.isArray(element)) return element.flatMap(buttons);
  if (element?.type === 'button') return [element];
  return element?.props ? buttons(element.props.children) : [];
}

function menu(data, onAgent = () => {}, atLimit = false) {
  return AgentSessionMenuItems({ data, onAgent, atLimit, onTerminal: () => {} });
}

test('menu renders eight recent sessions with metadata and only available new CLIs', () => {
  const sessions = [claude, codex, ...Array.from({ length: 7 }, (_, index) => ({ ...claude, id: String(index), title: `历史记录 ${index + 3}` }))];
  const markup = renderToStaticMarkup(menu({ sessions, tools: ['codex'] }));
  assert.match(markup, /新终端/);
  assert.match(markup, /新 Codex 会话/);
  assert.doesNotMatch(markup, /新 Claude 会话/);
  assert.match(markup, /继续之前的 AI 会话 · 这个项目/);
  assert.match(markup, /筛选条件进 URL/);
  assert.match(markup, /导出 CSV 加时间范围/);
  assert.match(markup, /48 分钟 · 4 个文件/);
  assert.equal(markup.match(/个文件/g).length, 7);
  assert.equal(markup.match(/data-agent-session-option/g).length, 8);
  assert.doesNotMatch(markup, /历史记录 9/);
  assert.doesNotMatch(renderToStaticMarkup(menu({ sessions: [], tools: [] })), /继续之前的 AI 会话/);
  assert.equal(agentSessionDate(claude.lastAt, new Date(2026, 10, 21, 12)), '昨天 23:40');
});

test('each history row generates the correct CLI resume command', () => {
  const commands = [];
  const options = buttons(menu({ sessions: [claude, codex], tools: ['claude', 'codex'] }, argv => commands.push(argv.join(' '))));
  for (const option of options.slice(1)) option.props.onClick();
  assert.deepEqual(commands, ['claude', 'codex', `claude --resume ${claude.id}`, `codex resume ${codex.id}`]);
});

test('history selection at the limit activates the already-running terminal reported by Bento', async t => {
  const invoked = [], activated = [];
  const current = { projectId: 'nested-project', slot: 3, sessionId: 'already-open', alive: true };
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { __TAURI_INTERNALS__: {
    invoke: async (command, args) => { invoked.push({ command, args }); return { session: current }; },
  } } });
  t.after(() => { delete globalThis.window; });
  const options = buttons(menu({ sessions: [codex], tools: ['codex'] }, argv =>
    activateAgentTerminal('project', argv, terminal => activated.push(terminal),
      async () => assert.fail('The running terminal needs no new launch')), true));
  assert.equal(options[0].props.disabled, true);
  assert.equal(options[1].props.disabled, true);
  assert.ok(!options[2].props.disabled);
  await options[2].props.onClick();
  assert.deepEqual(invoked, [{ command: 'terminal_agent_start', args: { projectId: 'project', argv: ['codex', 'resume', codex.id], recovery: undefined } }]);
  assert.deepEqual(activated, [current]);
});

test('the recovery card continues in the original tab or dismisses without resuming', async t => {
  const invoked = [], selected = [], prepared = [];
  const recovery = { slot: 2, sessionId: 'restored-tab' };
  const session = { ...recovery, projectId: 'project' };
  let finishPreparing;
  const ready = new Promise(resolve => { finishPreparing = resolve; });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { __TAURI_INTERNALS__: {
    invoke: async (command, args) => {
      invoked.push({ command, args });
      if (command === 'terminal_agent_start') return { session, launchId: 'prepared-launch' };
      assert.deepEqual(prepared, [session]);
      return session;
    },
  } } });
  t.after(() => { delete globalThis.window; });
  let visible = true;
  const card = AgentRecoveryCard({ session: claude, busy: false,
    onContinue: argv => activateAgentTerminal('project', argv, terminal => selected.push(terminal), async terminal => {
      assert.deepEqual(selected, [terminal]);
      await ready;
      prepared.push(terminal);
    }, recovery),
    onIgnore: () => { visible = false; },
  });
  assert.match(renderToStaticMarkup(card), /这里上次是 Claude 会话「筛选条件进 URL」/);
  const [continueButton, ignoreButton] = buttons(card);
  ignoreButton.props.onClick();
  assert.equal(visible, false);
  assert.deepEqual(invoked, []);
  const continuing = continueButton.props.onClick();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(invoked.map(({ command }) => command), ['terminal_agent_start']);
  finishPreparing();
  await continuing;
  assert.deepEqual(invoked, [
    { command: 'terminal_agent_start', args: { projectId: 'project', argv: ['claude', '--resume', claude.id], recovery } },
    { command: 'terminal_agent_launch', args: { projectId: 'project', ...recovery, launchId: 'prepared-launch' } },
  ]);
  assert.deepEqual(selected, [session]);
});

const projects = [{ id: 'records', name: 'Records' }, { id: 'reports', name: 'Reports' }];
const paletteOptions = (query, sessions, open = () => {}) => quickSwitcherSessionOptions(projects, sessions, query, open);

test('empty and whitespace-only palette queries omit history even after the index has loaded', () => {
  const sessions = [{ ...claude, projectId: 'records' }, { ...codex, projectId: 'reports' }];
  assert.deepEqual(paletteOptions('', sessions), []);
  assert.deepEqual(paletteOptions('   ', sessions), []);
});

test('palette matches session titles with project-name-search Unicode highlights across registered projects', () => {
  const sessions = [
    { ...claude, projectId: 'records', title: '𠀀导出 CSV' },
    { ...codex, projectId: 'reports', title: '导出 CSV 历史' },
    { ...claude, projectId: 'removed', title: '导出 CSV' },
    { ...codex, projectId: 'records', title: '调整字段', cwd: '/local/CSV' },
  ];
  const options = paletteOptions(' cSv ', sessions);
  assert.deepEqual(options.map(({ project }) => project.id), ['records', 'reports']);
  assert.deepEqual(options.map(({ indices }) => indices), [[4, 5, 6], [3, 4, 5]]);
  assert.deepEqual(paletteOptions('𠀀 cV', sessions)[0].indices, [0, 4, 6]);
  assert.deepEqual(paletteOptions('Reports', sessions), []);
  assert.deepEqual(paletteOptions('Codex', sessions), []);
});

test('palette limits matching history to the newest eight across projects even when a newer match is fuzzy', () => {
  const sessions = Array.from({ length: 12 }, (_, index) => ({
    ...claude, id: String(index), projectId: projects[index % 2].id, lastAt: index * 100,
    title: index === 11 ? 'daily account totals' : 'dat report',
  }));
  sessions.push({ ...codex, projectId: 'reports', title: 'CSV', lastAt: 2000 });
  sessions.push({ ...codex, projectId: 'removed', title: 'dat', lastAt: 3000 });
  const options = paletteOptions('dat', sessions);
  assert.deepEqual(options.map(({ session }) => session.id), ['11', '10', '9', '8', '7', '6', '5', '4']);
});

test('palette matches pinyin, initials and mixed text after direct title matches', () => {
  const direct = { ...codex, projectId: 'reports', title: 'sxtj export', lastAt: 1 };
  const chinese = { ...claude, projectId: 'records', title: '筛选条件进 URL', lastAt: 2 };
  const sessions = [chinese, direct];
  for (const query of ['shaixuan', 'shaixuan条件', '筛选tj', 'sxtjjurl', 'SHAIXUAN 条件进URL']) {
    assert.deepEqual(paletteOptions(query, sessions).map(row => row.session.id), [chinese.id], query);
  }
  const options = paletteOptions('sxtj', sessions);
  assert.deepEqual(options.map(row => row.session.id), [direct.id, chinese.id]);
  assert.deepEqual(options.map(row => row.indices), [[0, 1, 2, 3], []]);
  assert.deepEqual(paletteOptions('筛选', sessions)[0].indices, [0, 1]);
});

test('history publishes the startup cache while its background refresh is still pending', async t => {
  const requests = [], published = [];
  let finishRefresh;
  const refresh = new Promise(resolve => { finishRefresh = resolve; });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { __TAURI_INTERNALS__: {
    invoke: (command, args) => {
      requests.push({ command, args });
      return args.cached ? Promise.resolve([claude]) : refresh;
    },
  } } });
  t.after(() => { delete globalThis.window; });
  const command = 'terminal_agent_project_sessions';
  const loaded = loadAgentIndex(command, {}, result => published.push(result));
  await new Promise(setImmediate);
  assert.deepEqual(published, [[claude]]);
  assert.deepEqual(requests, [{ command, args: { cached: true } }, { command, args: {} }]);
  finishRefresh([codex]);
  await loaded;
  assert.deepEqual(published, [[claude], [codex]]);
});

test('palette arrows cross groups and Enter calls the same resume path as the plus menu, selecting the returned tab', async t => {
  const invoked = [], selected = [], pending = [];
  const running = { projectId: 'records', slot: 2, sessionId: 'running-tab', alive: true };
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { __TAURI_INTERNALS__: {
    invoke: async (command, args) => { invoked.push({ command, args }); return { session: running }; },
  } } });
  t.after(() => { delete globalThis.window; });

  const openAgent = (projectId, argv) => pending.push(activateAgentTerminal(projectId, argv, tab => selected.push(tab),
    async () => assert.fail('The running terminal needs no new launch')));
  const session = { ...codex, projectId: 'reports' };
  const options = [{ run: () => assert.fail('project activation') }, ...paletteOptions('CSV', [session], openAgent)];
  const index = quickSwitcherResultKey('ArrowDown', false, 0, options);
  assert.equal(index, 1);
  assert.equal(quickSwitcherResultKey('ArrowUp', false, index, options), 0);
  quickSwitcherResultKey('Enter', false, index, options);
  quickSwitcherResultKey('Enter', true, index, options);
  const menuRow = buttons(menu({ sessions: [session], tools: [] }, argv => openAgent('reports', argv)))[1];
  menuRow.props.onClick();
  await Promise.all(pending);

  const invocation = { command: 'terminal_agent_start', args: { projectId: 'reports', argv: ['codex', 'resume', codex.id], recovery: undefined } };
  assert.deepEqual(invoked, [invocation, invocation]);
  assert.deepEqual(selected, [running, running]);
});
