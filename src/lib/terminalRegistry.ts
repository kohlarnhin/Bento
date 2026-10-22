/// <reference types="vite/client" />
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview, type DragDropEvent } from '@tauri-apps/api/webview';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import '@fontsource/jetbrains-mono/latin-400.css';
import '@fontsource/jetbrains-mono/latin-700.css';
import type { TerminalAttention, TerminalOpenResult, TerminalOutput, TerminalSessionInfo } from '../types';
import { formatDroppedPaths, recoverShiftedImeInput, trackBareShift, transformTypedInput, type TerminalImeState } from './terminalInput';
import { DEFAULT_TERMINAL_FONT_SIZE, TERMINAL_FONT_FAMILY, TERMINAL_LINE_HEIGHT, TERMINAL_THEME } from './terminalAppearance';

type TerminalExit = Pick<TerminalSessionInfo, 'projectId' | 'sessionId' | 'exitCode'>;
export interface TerminalStatus extends Partial<TerminalSessionInfo> {
  opening?: boolean;
  error?: string;
}

interface InputChunk {
  data: string;
  binary: boolean;
  sessionId: string;
}

interface Entry {
  projectId: string;
  terminal: Terminal;
  fit: FitAddon;
  container: HTMLDivElement;
  opened: boolean;
  sessionId?: string;
  alive: boolean;
  seq: number;
  hydrating: boolean;
  buffered: TerminalOutput[];
  exits: TerminalExit[];
  attentions: TerminalAttention[];
  opening?: Promise<void>;
  input: InputChunk[];
  writing: boolean;
  resize?: { cols: number; rows: number };
  resizing: boolean;
  sentSize?: string;
  lastPrintable: string;
  pasting: boolean;
  ime: TerminalImeState;
  imePendingTimer?: ReturnType<typeof setTimeout>;
}

interface Registry {
  entries: Map<string, Entry>;
  snapshot: Record<string, TerminalStatus>;
  subscribers: Set<() => void>;
  initialized?: Promise<void>;
  showError: (message: string) => void;
}

// Retain instances and listeners through module HMR as well as React StrictMode remounts.
const registry: Registry = import.meta.hot?.data.terminalRegistry ?? {
  entries: new Map(), snapshot: {}, subscribers: new Set(), showError: () => {},
};
if (import.meta.hot) {
  import.meta.hot.data.terminalRegistry = registry;
  import.meta.hot.accept();
}

function publish(projectId: string, status: TerminalStatus) {
  registry.snapshot = { ...registry.snapshot, [projectId]: status };
  registry.subscribers.forEach((subscriber) => subscriber());
}

export function subscribeTerminals(subscriber: () => void) {
  registry.subscribers.add(subscriber);
  return () => { registry.subscribers.delete(subscriber); };
}

export const getTerminalSnapshot = () => registry.snapshot;
export const getTerminal = (projectId: string) => registry.entries.get(projectId)?.terminal;

let terminalFontSize = DEFAULT_TERMINAL_FONT_SIZE;
let terminalFonts: Promise<void> | undefined;

function loadTerminalFonts(): Promise<void> {
  if (!terminalFonts) {
    // fonts.ready alone can resolve before an unused face has been requested.
    terminalFonts = Promise.all([400, 700].map((weight) =>
      document.fonts.load(`${weight} ${DEFAULT_TERMINAL_FONT_SIZE}px "JetBrains Mono"`),
    )).then(async (faces) => {
      if (faces.some((weight) => weight.length === 0)) throw new Error('JetBrains Mono 字体未加载');
      await document.fonts.ready;
      // Also re-fit retained instances if the bundled faces finish loading after a remount.
      requestAnimationFrame(() => registry.entries.forEach((entry) => fitTerminal(entry.projectId)));
    }).catch((error) => {
      terminalFonts = undefined;
      throw error;
    });
  }
  return terminalFonts;
}

export function setTerminalFontSize(fontSize: number) {
  if (!Number.isFinite(fontSize) || fontSize <= 0) return;
  terminalFontSize = fontSize;
  registry.entries.forEach((entry) => {
    entry.terminal.options.fontSize = fontSize;
    // Detached terminals retain the option and fit when they are attached again.
    fitTerminal(entry.projectId);
  });
}

function receiveOutput(entry: Entry, output: TerminalOutput) {
  if (output.sessionId !== entry.sessionId || output.seq <= entry.seq) return;
  entry.seq = output.seq;
  // Output never passes through React, including when the container is detached.
  entry.terminal.write(output.data);
}

function receiveExit(entry: Entry, event: TerminalExit) {
  if (entry.sessionId !== event.sessionId) return;
  entry.alive = false;
  publish(entry.projectId, { ...registry.snapshot[entry.projectId], ...event, alive: false, attention: false });
}

function receiveAttention(event: TerminalAttention) {
  const current = registry.snapshot[event.projectId];
  if (current?.sessionId && current.sessionId !== event.sessionId) return;
  if (current?.alive === false && event.attention) return;
  publish(event.projectId, { ...current, ...event });
}

export function initTerminalRegistry(showError: (message: string) => void): Promise<void> {
  registry.showError = showError;
  if (!registry.initialized) {
    registry.initialized = (async () => {
      // Install listeners before any open/snapshot request, to avoid a rehydration gap.
      const unlistenOutput = await listen<TerminalOutput>('terminal-output', ({ payload }) => {
        const entry = registry.entries.get(payload.projectId);
        if (!entry) return;
        if (entry.hydrating) entry.buffered.push(payload);
        else receiveOutput(entry, payload);
      });
      try {
        const unlistenExit = await listen<TerminalExit>('terminal-exit', ({ payload }) => {
          const entry = registry.entries.get(payload.projectId);
          if (entry?.hydrating) entry.exits.push(payload);
          else if (entry) receiveExit(entry, payload);
          else if (registry.snapshot[payload.projectId]?.sessionId === payload.sessionId) {
            publish(payload.projectId, { ...registry.snapshot[payload.projectId], ...payload, alive: false, attention: false });
          }
        });
        const pending: TerminalAttention[] = [];
        let listing = true;
        let unlistenAttention: (() => void) | undefined;
        try {
          unlistenAttention = await listen<TerminalAttention>('terminal-attention', ({ payload }) => {
            const entry = registry.entries.get(payload.projectId);
            if (listing) pending.push(payload);
            else if (entry?.hydrating) entry.attentions.push(payload);
            else receiveAttention(payload);
          });
          const sessions = await invoke<TerminalSessionInfo[]>('terminal_list');
          sessions.forEach((session) => publish(session.projectId, session));
          listing = false;
          pending.forEach(receiveAttention);
        } catch (error) {
          unlistenAttention?.();
          unlistenExit();
          throw error;
        }
      } catch (error) {
        unlistenOutput();
        throw error;
      }
      await getCurrentWebview().onDragDropEvent(({ payload }) => handleTerminalDragDrop(payload));
    })().catch((error) => {
      registry.showError(`终端初始化失败：${error}`);
      throw error;
    });
  }
  return registry.initialized;
}

async function drainInput(entry: Entry) {
  if (entry.writing) return;
  entry.writing = true;
  try {
    while (entry.input.length) {
      const chunk = entry.input.shift()!;
      if (chunk.sessionId !== entry.sessionId || !entry.alive) continue;
      if (chunk.binary) {
        await invoke('terminal_write_bytes', {
          projectId: entry.projectId,
          data: Array.from(chunk.data, (character) => character.charCodeAt(0) & 255),
        });
      } else {
        await invoke('terminal_write', { projectId: entry.projectId, data: chunk.data });
      }
    }
  } catch (error) {
    entry.input = [];
    registry.showError(`终端输入失败：${error}`);
  } finally {
    entry.writing = false;
  }
}

function queueInput(entry: Entry, data: string, binary = false) {
  if (!entry.alive || !entry.sessionId || entry.hydrating) return;
  const last = entry.input[entry.input.length - 1];
  if (last && last.binary === binary && last.sessionId === entry.sessionId) last.data += data;
  else entry.input.push({ data, binary, sessionId: entry.sessionId });
  void drainInput(entry);
}

function createEntry(projectId: string): Entry {
  const terminal = new Terminal({
    theme: TERMINAL_THEME,
    fontFamily: TERMINAL_FONT_FAMILY,
    fontSize: terminalFontSize,
    lineHeight: TERMINAL_LINE_HEIGHT,
    allowProposedApi: true,
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 5000,
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);
  terminal.loadAddon(new Unicode11Addon());
  terminal.unicode.activeVersion = '11';
  terminal.loadAddon(new WebLinksAddon((event, url) => {
    event.preventDefault();
    void invoke('open_url', { url }).catch((error) => registry.showError(`打开链接失败：${error}`));
  }));
  const container = document.createElement('div');
  container.className = 'terminal-container';
  container.dataset.terminalProject = projectId;
  const entry: Entry = {
    projectId, terminal, fit, container, opened: false, alive: false, seq: 0,
    hydrating: false, buffered: [], exits: [], attentions: [], input: [], writing: false, resizing: false,
    lastPrintable: '', pasting: false,
    ime: { shiftDown: false, composing: false, pendingSend: false },
  };
  const paste = terminal.paste.bind(terminal);
  terminal.paste = (text) => {
    entry.lastPrintable = '';
    entry.pasting = true;
    try { paste(text); } finally { entry.pasting = false; }
  };
  container.addEventListener('paste', (event) => {
    const clipboard = event.clipboardData;
    if (!clipboard) return;
    const text = clipboard.getData('text/plain');
    const imageOnly = !text && Array.from(clipboard.items).some((item) => item.type.startsWith('image/'));
    event.preventDefault();
    event.stopImmediatePropagation();
    entry.lastPrintable = '';
    if (imageOnly) queueInput(entry, '\x16');
    else terminal.paste(text);
    terminal.focus();
  }, true);
  container.addEventListener('input', (event) => {
    if (event.target !== terminal.textarea) return;
    const data = recoverShiftedImeInput(entry.ime, event as InputEvent);
    // xterm rejects this input after Shift; its later 229 snapshot already includes the text.
    // Leave the event and textarea intact so all other input stays on xterm's normal path.
    if (data) terminal.input(data, true);
  }, true);
  terminal.attachCustomKeyEventHandler((event) => handleTerminalKey(entry, event));
  terminal.onData((data) => {
    if (entry.pasting) queueInput(entry, data);
    else {
      const input = transformTypedInput(data, entry.lastPrintable);
      entry.lastPrintable = input.lastPrintable;
      queueInput(entry, input.data);
    }
  });
  terminal.onBinary((data) => {
    // xterm sends legacy mouse reports through onBinary instead of onData.
    entry.lastPrintable = transformTypedInput(data, entry.lastPrintable).lastPrintable;
    queueInput(entry, data, true);
  });
  registry.entries.set(projectId, entry);
  return entry;
}

function handleTerminalKey(entry: Entry, event: KeyboardEvent): boolean {
  entry.ime.shiftDown = trackBareShift(entry.ime.shiftDown, event);
  if (event.isComposing || event.keyCode === 229) return true;
  const { key, metaKey, altKey, ctrlKey, shiftKey } = event;
  const command = metaKey && !altKey && !ctrlKey;
  // Let the native Edit menu generate the paste event, including image-only clipboards.
  if (command && key.toLowerCase() === 'v') {
    if (event.type === 'keydown') entry.lastPrintable = '';
    return false;
  }
  let data: string | undefined;
  if (shiftKey && !metaKey && !altKey && !ctrlKey && key === 'Enter') data = '\x1b\r';
  else if (altKey && !metaKey && !ctrlKey) {
    if (key === 'ArrowLeft') data = '\x1bb';
    if (key === 'ArrowRight') data = '\x1bf';
    if (key === 'Backspace') data = '\x1b\x7f';
  } else if (command) {
    if (key === 'ArrowLeft') data = '\x01';
    if (key === 'ArrowRight') data = '\x05';
    if (key === 'Backspace') data = '\x15';
  }
  const clear = command && key.toLowerCase() === 'k';
  const copy = command && key.toLowerCase() === 'c';
  if (data !== undefined || clear || copy) {
    event.preventDefault();
    if (event.type === 'keydown') {
      entry.lastPrintable = '';
      if (data !== undefined) queueInput(entry, data);
      else if (clear) entry.terminal.clear();
      else if (entry.terminal.hasSelection()) {
        void navigator.clipboard.writeText(entry.terminal.getSelection())
          .catch((error) => registry.showError(`复制失败：${error}`));
      }
    }
    // Swallow keypress/keyup too; no second Enter may reach xterm.
    return false;
  }
  if (event.type === 'keydown' && (ctrlKey || metaKey || ['Enter', 'Backspace', 'Delete', 'Escape', 'Tab', 'Home', 'End', 'PageUp', 'PageDown'].includes(key) || key.startsWith('Arrow'))) {
    entry.lastPrintable = '';
  }
  return true;
}

/** Tauri reports window-wide physical coordinates, including drops on the sidebar. */
export function handleTerminalDragDrop(event: DragDropEvent) {
  let target: Entry | undefined;
  registry.entries.forEach((entry) => {
    const pane = entry.container.closest<HTMLElement>('[data-terminal-pane]');
    if (!pane) return;
    delete pane.dataset.fileDrag;
    if (event.type === 'leave' || !visible(entry)) return;
    const rect = pane.querySelector('[data-terminal-area]')!.getBoundingClientRect();
    const x = event.position.x / window.devicePixelRatio;
    const y = event.position.y / window.devicePixelRatio;
    if (x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom) target = entry;
  });
  if (!target) return;
  if (event.type === 'enter' || event.type === 'over') {
    target.container.closest<HTMLElement>('[data-terminal-pane]')!.dataset.fileDrag = 'true';
  } else if (event.type === 'drop' && target.alive && !target.hydrating) {
    const text = formatDroppedPaths(event.paths);
    if (text) target.terminal.paste(text);
    target.terminal.focus();
  }
}

function visible(entry: Entry) {
  const rect = entry.container.getBoundingClientRect();
  return entry.container.isConnected && rect.width > 40 && rect.height > 40;
}

async function drainResize(entry: Entry) {
  if (entry.resizing || !entry.sessionId) return;
  entry.resizing = true;
  try {
    while (entry.resize) {
      const dimensions = entry.resize;
      entry.resize = undefined;
      const key = `${entry.sessionId}:${dimensions.cols}:${dimensions.rows}`;
      if (entry.sentSize === key) continue;
      await invoke('terminal_resize', { projectId: entry.projectId, ...dimensions });
      entry.sentSize = key;
    }
  } catch (error) {
    registry.showError(`调整终端大小失败：${error}`);
  } finally {
    entry.resizing = false;
  }
}

export function fitTerminal(projectId: string, focus = false) {
  const entry = registry.entries.get(projectId);
  if (!entry?.opened || !visible(entry)) return;
  // Focus also when the renderer is still measuring cells on the first visible frame.
  if (focus) entry.terminal.focus();
  const atBottom = entry.terminal.buffer.active.viewportY >= entry.terminal.buffer.active.baseY;
  const dimensions = entry.fit.proposeDimensions();
  if (!dimensions || !Number.isFinite(dimensions.cols) || !Number.isFinite(dimensions.rows)
      || dimensions.cols < 2 || dimensions.rows < 1) return;
  entry.fit.fit();
  entry.resize = { cols: entry.terminal.cols, rows: entry.terminal.rows };
  void drainResize(entry);
  entry.terminal.refresh(0, entry.terminal.rows - 1);
  if (atBottom) entry.terminal.scrollToBottom();
}

async function hydrate(entry: Entry, restart: boolean) {
  entry.hydrating = true;
  entry.buffered = [];
  entry.exits = [];
  entry.attentions = [];
  entry.input = [];
  entry.lastPrintable = '';
  publish(entry.projectId, { ...registry.snapshot[entry.projectId], opening: true, error: undefined });
  try {
    const result = await invoke<TerminalOpenResult>(restart ? 'terminal_restart' : 'terminal_open', {
      projectId: entry.projectId, cols: entry.terminal.cols, rows: entry.terminal.rows,
    });
    entry.sessionId = result.session.sessionId;
    entry.alive = result.session.alive;
    entry.seq = result.snapshot.seq;
    entry.terminal.reset();
    entry.terminal.write(result.snapshot.data);
    publish(entry.projectId, result.session);
    entry.buffered.sort((a, b) => a.seq - b.seq).forEach((output) => receiveOutput(entry, output));
    entry.exits.forEach((event) => receiveExit(entry, event));
    entry.attentions.forEach(receiveAttention);
    entry.buffered = [];
    entry.exits = [];
    entry.attentions = [];
  } catch (error) {
    const message = `打开终端失败：${error}`;
    // A failed restart may already have terminated the old shell. Reconcile before retrying.
    const sessions = await invoke<TerminalSessionInfo[]>('terminal_list').catch(() => undefined);
    const session = sessions?.find((item) => item.projectId === entry.projectId);
    if (sessions) {
      entry.sessionId = session?.sessionId;
      entry.alive = !!session?.alive;
    }
    entry.buffered.sort((a, b) => a.seq - b.seq).forEach((output) => receiveOutput(entry, output));
    entry.exits.forEach((event) => receiveExit(entry, event));
    publish(entry.projectId, { ...(sessions ? session : registry.snapshot[entry.projectId]), opening: false, error: message });
    entry.attentions.forEach(receiveAttention);
    registry.showError(message);
  } finally {
    entry.hydrating = false;
    entry.opening = undefined;
    fitTerminal(entry.projectId, true);
  }
}

async function showTerminal(entry: Entry) {
  await initTerminalRegistry(registry.showError);
  // A missing bundled face must not keep the terminal from opening; the fallback fonts still work.
  await loadTerminalFonts().catch((error) => console.warn('Terminal font:', error));
  if (!visible(entry)) return;
  if (!entry.opened) {
    entry.terminal.open(entry.container);
    const textarea = entry.terminal.textarea!;
    textarea.addEventListener('blur', () => { entry.ime.shiftDown = false; });
    textarea.addEventListener('compositionstart', () => { entry.ime.composing = true; });
    textarea.addEventListener('compositionend', () => {
      entry.ime.composing = false;
      entry.ime.pendingSend = true;
      clearTimeout(entry.imePendingTimer);
      // Registered after open(): xterm queues its composition finalization before this timer.
      entry.imePendingTimer = setTimeout(() => {
        entry.ime.pendingSend = false;
        entry.imePendingTimer = undefined;
      }, 0);
    });
    entry.opened = true;
  }
  fitTerminal(entry.projectId, true);
  if (!entry.sessionId && !entry.opening) entry.opening = hydrate(entry, false);
}

export function attachTerminal(projectId: string, host: HTMLDivElement) {
  const entry = registry.entries.get(projectId) ?? createEntry(projectId);
  host.appendChild(entry.container);
  void showTerminal(entry).catch((error) => registry.showError(`终端显示失败：${error}`));
  const frame = requestAnimationFrame(() => fitTerminal(projectId, true));
  return () => {
    cancelAnimationFrame(frame);
    // A detached container and its xterm instance keep receiving output for the webview's lifetime.
    if (entry.container.parentElement === host) entry.container.remove();
  };
}

export async function restartTerminal(projectId: string) {
  await initTerminalRegistry(registry.showError);
  const entry = registry.entries.get(projectId) ?? createEntry(projectId);
  if (entry.opening) return entry.opening;
  entry.opening = hydrate(entry, true);
  return entry.opening;
}

export function retryTerminal(projectId: string) {
  const entry = registry.entries.get(projectId);
  if (entry) void showTerminal(entry).catch((error) => registry.showError(String(error)));
}
