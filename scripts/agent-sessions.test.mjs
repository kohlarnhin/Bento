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
const { activateAgentTerminal, agentSessionDate } = await import('../src/lib/agentSessions.ts');
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
    invoke: async (command, args) => { invoked.push({ command, args }); return current; },
  } } });
  t.after(() => { delete globalThis.window; });
  const options = buttons(menu({ sessions: [codex], tools: ['codex'] }, argv =>
    activateAgentTerminal('project', argv, terminal => activated.push(terminal)), true));
  assert.equal(options[0].props.disabled, true);
  assert.equal(options[1].props.disabled, true);
  assert.ok(!options[2].props.disabled);
  await options[2].props.onClick();
  assert.deepEqual(invoked, [{ command: 'terminal_agent_start', args: { projectId: 'project', argv: ['codex', 'resume', codex.id], recovery: undefined } }]);
  assert.deepEqual(activated, [current]);
});

test('the recovery card continues in the original tab or dismisses without resuming', async t => {
  const invoked = [], selected = [];
  const recovery = { slot: 2, sessionId: 'restored-tab' };
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { __TAURI_INTERNALS__: {
    invoke: async (command, args) => { invoked.push({ command, args }); return { ...recovery, projectId: 'project' }; },
  } } });
  t.after(() => { delete globalThis.window; });
  let visible = true;
  const card = AgentRecoveryCard({ session: claude, busy: false,
    onContinue: argv => activateAgentTerminal('project', argv, terminal => selected.push(terminal), recovery),
    onIgnore: () => { visible = false; },
  });
  assert.match(renderToStaticMarkup(card), /这里上次是 Claude 会话「筛选条件进 URL」/);
  const [continueButton, ignoreButton] = buttons(card);
  ignoreButton.props.onClick();
  assert.equal(visible, false);
  assert.deepEqual(invoked, []);
  await continueButton.props.onClick();
  assert.deepEqual(invoked, [{ command: 'terminal_agent_start', args: { projectId: 'project', argv: ['claude', '--resume', claude.id], recovery } }]);
  assert.deepEqual(selected, [{ ...recovery, projectId: 'project' }]);
});
