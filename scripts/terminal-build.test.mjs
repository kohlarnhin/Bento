import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { build } from 'vite';

test('the production terminal bundle answers a split mode query and keeps parsing output', async () => {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const entry = fileURLToPath(new URL('../terminal-build-fixture.js', import.meta.url));
  const [bundle] = await build({
    root,
    logLevel: 'silent',
    plugins: [{
      name: 'terminal-build-fixture',
      resolveId: id => id === entry ? '\0terminal-build-fixture' : null,
      load: id => id === '\0terminal-build-fixture' ? "export { Terminal } from '@xterm/xterm';" : null,
    }],
    build: { write: false, lib: { entry, name: 'TerminalBuildFixture', formats: ['iife'] } },
  });
  const { code } = bundle.output.find(chunk => chunk.type === 'chunk' && chunk.isEntry);
  const callbacks = [];
  const browser = {
    document: {}, navigator, performance, queueMicrotask, setTimeout: callback => callbacks.push(callback),
  };
  browser.window = browser;
  const { Terminal } = runInNewContext(`${code}\nTerminalBuildFixture`, browser);
  const terminal = new Terminal({ allowProposedApi: true });
  try {
    const replies = [];
    terminal.onData(data => replies.push(data));
    // Codex's DEC mode query can be split across two PTY reads.
    terminal.write('\x1b[?10');
    terminal.write('07$p\x1b[c\x1b[6n');
    terminal.write('terminal output');
    while (callbacks.length) callbacks.shift()();

    assert.deepEqual(replies, ['\x1b[?1007;0$y', '\x1b[?1;2c', '\x1b[1;1R']);
    assert.equal(terminal.buffer.active.getLine(0).translateToString(true), 'terminal output');
  } finally {
    terminal.dispose();
  }
});
