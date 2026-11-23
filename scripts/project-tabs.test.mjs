import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

// Use the production React renderer for markup assertions on components with layout effects.
process.env.NODE_ENV = 'production';
const { createElement } = await import('react');
const { renderToStaticMarkup } = await import('react-dom/server');
const { Provider } = await import('@radix-ui/react-tooltip');

function radios(markup) {
  return [...markup.matchAll(/<button\b[^>]*role="radio"([^>]*)>(.*?)<\/button>/gs)].map(([, attributes, contents]) => ({
    label: /data-tab-label="([^"]+)"/.exec(contents)?.[1],
    checked: /aria-checked="([^"]+)"/.exec(attributes)?.[1],
    tabIndex: /tabindex="([^"]+)"/.exec(attributes)?.[1],
  }));
}

const radio = (label, selected = false) => ({ label, checked: String(selected), tabIndex: selected ? '0' : '-1' });

test('project view segments', async t => {
  const vite = await createServer({
    root: fileURLToPath(new URL('../', import.meta.url)),
    logLevel: 'silent',
    optimizeDeps: { noDiscovery: true },
    server: { middlewareMode: true, ws: false, hmr: false, watch: null },
  });
  t.after(() => vite.close());
  const { ProjectTabs } = await vite.ssrLoadModule('/src/components/ProjectTabs.tsx');
  const noop = () => {};
  function render(view, slots, activeSlot = 1) {
    const terminals = Object.fromEntries(slots.map(slot => [`project#${slot}`, { sessionId: String(slot), alive: true }]));
    return renderToStaticMarkup(createElement(Provider, null, createElement(ProjectTabs, {
      projectId: 'project', view, slots, activeSlot, terminals, maxTerminals: 4,
      onSelectView: noop, onSelectTerminal: noop, onNewTerminal: async () => {}, onOpenAgent: noop,
      closeActiveTab: { current: null },
    })));
  }

  await t.test('offers a single terminal segment beside Overview and Git when empty', () => {
    const overview = render('overview', []);
    assert.match(overview, /role="radiogroup" aria-label="项目视图"/);
    assert.deepEqual(radios(overview), [radio('概览', true), radio('终端'), radio('Git')]);
    assert.deepEqual(radios(render('git', [])), [radio('概览'), radio('终端'), radio('Git', true)]);
    assert.doesNotMatch(overview, /data-terminal-tab-close/);
  });

  await t.test('replaces the empty segment with the first terminal and its controls', () => {
    const terminal = render('terminal', [1]);
    assert.deepEqual(radios(terminal), [radio('概览'), radio('终端 1', true), radio('Git')]);
    assert.match(terminal, /aria-keyshortcuts="Meta\+T"[^>]*data-terminal-tab-add/);
    assert.match(terminal, /aria-label="关闭终端 1"/);
  });

  await t.test('preserves existing terminal order, names, and selection', () => {
    const terminal = render('terminal', [2, 1], 1);
    assert.deepEqual(radios(terminal), [radio('概览'), radio('终端 1'), radio('终端 2', true), radio('Git')]);
    assert.match(terminal, /aria-label="关闭终端 1" data-terminal-tab-close="2"/);
    assert.match(terminal, /aria-label="关闭终端 2" data-terminal-tab-close="1"/);
  });
});
