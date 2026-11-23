/// <reference types="vite/client" />
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview, type DragDropEvent } from '@tauri-apps/api/webview';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { SearchAddon } from '@xterm/addon-search';
import '@xterm/xterm/css/xterm.css';
import type { TerminalAgentEvent, TerminalAttention, TerminalCompletionEvent, TerminalOpenResult, TerminalOutput, TerminalSessionInfo } from '../types';
import { formatDroppedPaths, recoverShiftedImeInput, trackBareShift, transformTypedInput, type TerminalImeState } from './terminalInput';
import { DEFAULT_TERMINAL_FONT_SIZE, TERMINAL_FALLBACK_FONT_FAMILY, TERMINAL_FONT_FAMILY, TERMINAL_LINE_HEIGHT, TERMINAL_THEME } from './terminalAppearance';
import { createTitlePublisher, projectTerminalSlots } from './terminalUsability';

type TerminalClosed = Pick<TerminalSessionInfo, 'projectId' | 'slot' | 'sessionId'>;
export interface TerminalStatus extends Partial<TerminalSessionInfo> {
  projectId: string;
  slot: number;
  opening?: boolean;
  error?: string;
  title?: string;
  attentionAt?: number;
}

interface InputChunk {
  data: string;
  binary: boolean;
  sessionId: string;
}

interface Entry {
  projectId: string;
  slot: number;
  disposed: boolean;
  expectedSessionId?: string;
  terminal: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  title: ReturnType<typeof createTitlePublisher>;
  container: HTMLDivElement;
  opened: boolean;
  sessionId?: string;
  alive: boolean;
  seq: number;
  hydrating: boolean;
  buffered: TerminalOutput[];
  attentions: TerminalAttention[];
  completions: TerminalCompletionEvent[];
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
  inactiveHost?: HTMLDivElement;
  restoredSlots: Map<string, number>;
  snapshot: Record<string, TerminalStatus>;
  subscribers: Set<() => void>;
  initialized?: Promise<void>;
  closedSessions: Set<string>;
  agentEvents: Map<string, TerminalAgentEvent>;
  fontSize: number;
  showError: (message: string) => void;
}

// Retain instances and listeners through module HMR as well as React StrictMode remounts.
const registry: Registry = import.meta.hot?.data.terminalRegistry ?? {
  entries: new Map(), snapshot: {}, subscribers: new Set(), closedSessions: new Set(), restoredSlots: new Map(),
  agentEvents: new Map(), fontSize: DEFAULT_TERMINAL_FONT_SIZE, showError: () => {},
};
if (import.meta.hot) {
  registry.agentEvents ??= new Map();
  import.meta.hot.data.terminalRegistry = registry;
  import.meta.hot.accept();
}

export const terminalKey = (projectId: string, slot: number) => `${projectId}#${slot}`;

export function getProjectTerminalSlots(projectId: string, snapshot = registry.snapshot): number[] {
  return projectTerminalSlots(projectId, Object.values(snapshot));
}

function publish(status: TerminalStatus) {
  registry.snapshot = { ...registry.snapshot, [terminalKey(status.projectId, status.slot)]: status };
  registry.subscribers.forEach((subscriber) => subscriber());
}

export function subscribeTerminals(subscriber: () => void) {
  registry.subscribers.add(subscriber);
  return () => { registry.subscribers.delete(subscriber); };
}

export const getTerminalSnapshot = () => registry.snapshot;
export const getTerminal = (projectId: string, slot: number) => registry.entries.get(terminalKey(projectId, slot))?.terminal;
export const getTerminalSearch = (projectId: string, slot: number) => registry.entries.get(terminalKey(projectId, slot))?.search;

export function takeRestoredSlot(projectId: string) {
  const slot = registry.restoredSlots.get(projectId);
  registry.restoredSlots.delete(projectId);
  return slot !== undefined && registry.snapshot[terminalKey(projectId, slot)]?.sessionId ? slot : undefined;
}

let lastAttentionAt = 0;
function attentionTime(attention: boolean, previous?: TerminalStatus) {
  if (!attention) return undefined;
  if (previous?.attention && previous.attentionAt !== undefined) return previous.attentionAt;
  // Preserve ordering even when two events land in the same millisecond.
  return lastAttentionAt = Math.max(Date.now(), lastAttentionAt + 1);
}

let terminalFonts: Promise<void> | undefined;

function loadTerminalFonts(): Promise<void> {
  if (!terminalFonts) {
    // fonts.ready alone can resolve before an unused face has been requested.
    terminalFonts = Promise.all([400, 700].map((weight) =>
      document.fonts.load(`${weight} ${DEFAULT_TERMINAL_FONT_SIZE}px "JetBrains Mono"`),
    )).then(async (faces) => {
      if (faces.some((weight) => weight.length === 0)) throw new Error('JetBrains Mono 字体未加载');
      await document.fonts.ready;
      requestAnimationFrame(() => registry.entries.forEach((entry) => {
        if (!entry.opened || !visible(entry)) return;
        // Updating the family invalidates xterm's cached measurements if it opened on the fallback.
        entry.terminal.options.fontFamily = TERMINAL_FONT_FAMILY;
        fitTerminal(entry.projectId, entry.slot);
      }));
    }).catch((error) => {
      terminalFonts = undefined;
      throw error;
    });
  }
  return terminalFonts;
}

export function setTerminalFontSize(fontSize: number) {
  if (!Number.isFinite(fontSize) || fontSize <= 0) return;
  registry.fontSize = fontSize;
  registry.entries.forEach((entry) => {
    const atBottom = entry.terminal.buffer.active.viewportY >= entry.terminal.buffer.active.baseY;
    entry.terminal.options.fontSize = fontSize;
    // Inactive terminals retain the option and fit when they are shown again.
    fitTerminal(entry.projectId, entry.slot);
    if (atBottom) entry.terminal.scrollToBottom();
  });
}

function receiveOutput(entry: Entry, output: TerminalOutput) {
  if (output.sessionId !== entry.sessionId || output.seq <= entry.seq) return;
  entry.seq = output.seq;
  // Output never passes through React, including when the terminal is hidden.
  entry.terminal.write(output.data);
}

function receiveAttention(event: TerminalAttention) {
  const current = registry.snapshot[terminalKey(event.projectId, event.slot)];
  if (!current || current.sessionId !== event.sessionId) return;
  if (current.alive === false && event.attention) return;
  publish({ ...current, ...event, attentionAt: attentionTime(event.attention, current) });
}

function receiveCompletion(event: TerminalCompletionEvent) {
  const current = registry.snapshot[terminalKey(event.projectId, event.slot)];
  if (current?.sessionId === event.sessionId) publish({ ...current, completion: event.completion });
}

function receiveAgent(event: TerminalAgentEvent) {
  if (registry.closedSessions.has(event.sessionId)) return;
  const current = registry.snapshot[terminalKey(event.projectId, event.slot)];
  if (current?.sessionId === event.sessionId) {
    if (current.alive && event.agentRevision > (current.agentRevision ?? 0)) publish({ ...current, ...event });
  } else if (event.agentRevision > (registry.agentEvents.get(event.sessionId)?.agentRevision ?? 0)) {
    registry.agentEvents.set(event.sessionId, event);
  }
}

function mergeAgentSnapshot(session: TerminalSessionInfo): Pick<TerminalSessionInfo, 'agent' | 'agentRevision'> {
  const current = registry.snapshot[terminalKey(session.projectId, session.slot)];
  const buffered = registry.agentEvents.get(session.sessionId);
  registry.agentEvents.delete(session.sessionId);
  let latest = session;
  for (const source of [current, buffered]) {
    if (source?.sessionId === session.sessionId && (source.agentRevision ?? 0) > (latest.agentRevision ?? 0)) {
      latest = { ...latest, agent: source.agent, agentRevision: source.agentRevision };
    }
  }
  return { agent: session.alive ? latest.agent : null, agentRevision: latest.agentRevision };
}

function receiveOpened(session: TerminalSessionInfo) {
  if (registry.closedSessions.has(session.sessionId)) return;
  const key = terminalKey(session.projectId, session.slot);
  const entry = registry.entries.get(key);
  const current = registry.snapshot[key];
  const sameSession = current?.sessionId === session.sessionId;
  if (!sameSession) entry?.title.reset();
  if (entry) entry.expectedSessionId = session.sessionId;
  const attention = session.attention;
  publish({ ...session, ...mergeAgentSnapshot(session),
    title: sameSession ? current?.title ?? session.title : session.title,
    attentionAt: attentionTime(attention, sameSession ? current : undefined), opening: entry?.hydrating });
  if (entry && entry.sessionId !== session.sessionId && !entry.hydrating) startHydrate(entry, false);
}

function receiveClosed(event: TerminalClosed) {
  if (registry.closedSessions.has(event.sessionId)) return;
  registry.closedSessions.add(event.sessionId);
  registry.agentEvents.delete(event.sessionId);
  const key = terminalKey(event.projectId, event.slot);
  const current = registry.snapshot[key];
  const entry = registry.entries.get(key);
  // A delayed close of the old session must never dispose its replacement.
  const knownSession = entry?.expectedSessionId ?? current?.sessionId;
  if (knownSession && knownSession !== event.sessionId) return;
  if (registry.restoredSlots.get(event.projectId) === event.slot) registry.restoredSlots.delete(event.projectId);
  if (entry) {
    entry.disposed = true;
    entry.input = [];
    entry.buffered = [];
    entry.attentions = [];
    entry.completions = [];
    entry.resize = undefined;
    clearTimeout(entry.imePendingTimer);
    entry.title.reset();
    entry.terminal.dispose();
    entry.container.remove();
    registry.entries.delete(key);
  }
  const next = { ...registry.snapshot };
  delete next[key];
  registry.snapshot = next;
  registry.subscribers.forEach(subscriber => subscriber());
}

export function initTerminalRegistry(showError: (message: string) => void): Promise<void> {
  registry.showError = showError;
  if (!registry.initialized) {
    registry.initialized = (async () => {
      const unlisteners: (() => void)[] = [];
      const pending: (() => void)[] = [];
      let listing = true;
      const dispatch = (apply: () => void) => { if (listing) pending.push(apply); else apply(); };
      try {
        // Queue lifecycle events until the seed is installed; no list/event race can resurrect a closed tab.
        unlisteners.push(await listen<TerminalOutput>('terminal-output', ({ payload }) => dispatch(() => {
          const entry = registry.entries.get(terminalKey(payload.projectId, payload.slot));
          if (entry?.hydrating) entry.buffered.push(payload);
          else if (entry) receiveOutput(entry, payload);
        })));
        unlisteners.push(await listen<TerminalAttention>('terminal-attention', ({ payload }) => dispatch(() => {
          const entry = registry.entries.get(terminalKey(payload.projectId, payload.slot));
          if (entry?.hydrating) entry.attentions.push(payload);
          else receiveAttention(payload);
        })));
        unlisteners.push(await listen<TerminalCompletionEvent>('terminal-completion', ({ payload }) => dispatch(() => {
          const entry = registry.entries.get(terminalKey(payload.projectId, payload.slot));
          if (entry?.hydrating) entry.completions.push(payload);
          else receiveCompletion(payload);
        })));
        unlisteners.push(await listen<TerminalAgentEvent>('terminal-agent', ({ payload }) => dispatch(() => receiveAgent(payload))));
        unlisteners.push(await listen<TerminalSessionInfo>('terminal-opened', ({ payload }) => dispatch(() => receiveOpened(payload))));
        unlisteners.push(await listen<TerminalClosed>('terminal-closed', ({ payload }) => dispatch(() => receiveClosed(payload))));
        const sessions = await invoke<TerminalSessionInfo[]>('terminal_list');
        for (const session of sessions) {
          if (session.restoredActive) registry.restoredSlots.set(session.projectId, session.slot);
          receiveOpened(session);
        }
        listing = false;
        pending.forEach(apply => apply());
        unlisteners.push(await getCurrentWebview().onDragDropEvent(({ payload }) => handleTerminalDragDrop(payload)));
      } catch (error) {
        unlisteners.forEach(unlisten => unlisten());
        throw error;
      }
    })().catch((error) => {
      registry.initialized = undefined;
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
    while (!entry.disposed && entry.input.length) {
      const chunk = entry.input.shift()!;
      if (chunk.sessionId !== entry.sessionId || !entry.alive || entry.hydrating) continue;
      if (chunk.binary) {
        await invoke('terminal_write_bytes', {
          projectId: entry.projectId, slot: entry.slot,
          data: Array.from(chunk.data, (character) => character.charCodeAt(0) & 255),
        });
      } else {
        await invoke('terminal_write', { projectId: entry.projectId, slot: entry.slot, data: chunk.data });
      }
    }
  } catch (error) {
    entry.input = [];
    if (!entry.disposed && !entry.hydrating) registry.showError(`终端输入失败：${error}`);
  } finally {
    entry.writing = false;
  }
}

function queueInput(entry: Entry, data: string, binary = false) {
  if (entry.disposed || !entry.alive || !entry.sessionId || entry.hydrating) return;
  const last = entry.input[entry.input.length - 1];
  if (last && last.binary === binary && last.sessionId === entry.sessionId) last.data += data;
  else entry.input.push({ data, binary, sessionId: entry.sessionId });
  void drainInput(entry);
}

function createEntry(projectId: string, slot: number): Entry {
  const terminal = new Terminal({
    theme: TERMINAL_THEME,
    fontFamily: TERMINAL_FONT_FAMILY,
    fontSize: registry.fontSize,
    lineHeight: TERMINAL_LINE_HEIGHT,
    allowProposedApi: true,
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 5000,
  });
  const fit = new FitAddon();
  // Scrollback is bounded by the terminal; include every retained match.
  const search = new SearchAddon({ highlightLimit: Number.MAX_SAFE_INTEGER });
  terminal.loadAddon(fit);
  terminal.loadAddon(search);
  terminal.loadAddon(new Unicode11Addon());
  terminal.unicode.activeVersion = '11';
  terminal.loadAddon(new WebLinksAddon((event, url) => {
    event.preventDefault();
    void invoke('open_url', { url }).catch((error) => registry.showError(`打开链接失败：${error}`));
  }));
  const container = document.createElement('div');
  container.className = 'terminal-container';
  container.dataset.terminalProject = projectId;
  container.dataset.terminalSlot = String(slot);
  const entry: Entry = {
    projectId, slot, disposed: false, terminal, fit, search, container, opened: false, alive: false, seq: 0,
    title: createTitlePublisher(title => {
      const current = registry.snapshot[terminalKey(projectId, slot)];
      if (entry.disposed || !current || current.sessionId !== entry.sessionId || current.title === (title || undefined)) return;
      publish({ ...current, title: title || undefined });
    }),
    hydrating: false, buffered: [], attentions: [], completions: [], input: [], writing: false, resizing: false,
    lastPrintable: '', pasting: false,
    ime: { shiftDown: false, composing: false, pendingSend: false },
  };
  terminal.onTitleChange(title => entry.title.update(title));
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
  registry.entries.set(terminalKey(projectId, slot), entry);
  return entry;
}

function handleTerminalKey(entry: Entry, event: KeyboardEvent): boolean {
  entry.ime.shiftDown = trackBareShift(entry.ime.shiftDown, event);
  if (event.isComposing || event.keyCode === 229) return true;
  const { key, metaKey, altKey, ctrlKey, shiftKey } = event;
  const command = metaKey && !altKey && !ctrlKey;
  // Global project actions must never reach the PTY, including later key events.
  if (command && !shiftKey && ['p', 'n'].includes(key.toLowerCase())) {
    event.preventDefault();
    return false;
  }
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
    if (event.type === 'leave' || pane.closest('[inert]') || !visible(entry)) return;
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
  if (entry.disposed || entry.hydrating || entry.resizing || !entry.sessionId) return;
  entry.resizing = true;
  try {
    while (!entry.disposed && !entry.hydrating && entry.resize) {
      const dimensions = entry.resize;
      entry.resize = undefined;
      const key = `${entry.sessionId}:${dimensions.cols}:${dimensions.rows}`;
      if (entry.sentSize === key) continue;
      await invoke('terminal_resize', { projectId: entry.projectId, slot: entry.slot, ...dimensions });
      entry.sentSize = key;
    }
  } catch (error) {
    if (!entry.disposed) registry.showError(`调整终端大小失败：${error}`);
  } finally {
    entry.resizing = false;
  }
}

export function fitTerminal(projectId: string, slot: number, focus = false) {
  const entry = registry.entries.get(terminalKey(projectId, slot));
  if (!entry?.opened || entry.disposed || !visible(entry)) return;
  // Focus also when the renderer is still measuring cells on the first visible frame.
  if (focus) entry.terminal.focus();
  if (entry.container.closest('[data-terminal-layout-motion]')) return;
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

export function fitVisibleTerminals() {
  registry.entries.forEach(entry => fitTerminal(entry.projectId, entry.slot));
}

function startHydrate(entry: Entry, restart: boolean): Promise<void> {
  if (entry.opening) return entry.opening;
  const key = terminalKey(entry.projectId, entry.slot);
  const current = registry.snapshot[key];
  if (entry.disposed || !current?.sessionId) return Promise.resolve();
  entry.expectedSessionId = current.sessionId;
  entry.hydrating = true;
  entry.buffered = [];
  entry.attentions = [];
  entry.completions = [];
  entry.input = [];
  entry.resize = undefined;
  entry.lastPrintable = '';
  entry.title.reset();
  publish({ ...current, opening: true, error: undefined });
  entry.opening = hydrate(entry, restart).finally(() => {
    if (entry.disposed) return;
    entry.hydrating = false;
    entry.opening = undefined;
    fitTerminal(entry.projectId, entry.slot);
  });
  return entry.opening;
}

async function hydrate(entry: Entry, restart: boolean) {
  const key = terminalKey(entry.projectId, entry.slot);
  const sessionId = entry.expectedSessionId;
  if (!sessionId || entry.disposed) return;
  const args = { projectId: entry.projectId, slot: entry.slot, cols: entry.terminal.cols, rows: entry.terminal.rows };
  try {
    let result = await invoke<TerminalOpenResult>(restart ? 'terminal_restart' : 'terminal_open', {
      ...args, sessionId,
    });
    if (restart && entry.expectedSessionId === sessionId) entry.expectedSessionId = result.session.sessionId;
    // An external restart may overtake an in-flight snapshot. Keep buffering until it matches.
    while (!entry.disposed && entry.expectedSessionId && result.session.sessionId !== entry.expectedSessionId) {
      result = await invoke<TerminalOpenResult>('terminal_open', {
        ...args, sessionId: entry.expectedSessionId,
      });
    }
    if (entry.disposed || registry.closedSessions.has(result.session.sessionId)) return;
    // Flush queued old-session writes before resetting the renderer.
    await new Promise<void>(resolve => entry.terminal.write('', resolve));
    if (entry.disposed) return;
    if (entry.expectedSessionId && result.session.sessionId !== entry.expectedSessionId) return hydrate(entry, false);
    entry.sessionId = result.session.sessionId;
    entry.expectedSessionId = entry.sessionId;
    entry.alive = result.session.alive;
    entry.seq = result.snapshot.seq;
    entry.title.reset();
    entry.terminal.reset();
    entry.terminal.write(result.snapshot.data);
    publish({ ...result.session, ...mergeAgentSnapshot(result.session), attentionAt: attentionTime(result.session.attention, registry.snapshot[key]) });
    entry.buffered.sort((a, b) => a.seq - b.seq).forEach(output => receiveOutput(entry, output));
    entry.attentions.forEach(receiveAttention);
    entry.completions.forEach(receiveCompletion);
    entry.buffered = [];
    entry.attentions = [];
    entry.completions = [];
  } catch (error) {
    if (entry.disposed) return;
    const message = `打开终端失败：${error}`;
    const sessions = await invoke<TerminalSessionInfo[]>('terminal_list').catch(() => undefined);
    if (entry.disposed) return;
    const session = sessions?.find(item => item.projectId === entry.projectId && item.slot === entry.slot);
    if (sessions && !session) {
      receiveClosed({ projectId: entry.projectId, slot: entry.slot, sessionId: entry.expectedSessionId ?? sessionId });
      return;
    }
    if (session && session.sessionId !== sessionId) {
      entry.expectedSessionId = session.sessionId;
      return hydrate(entry, false);
    }
    entry.buffered.sort((a, b) => a.seq - b.seq).forEach(output => receiveOutput(entry, output));
    publish({ ...(sessions ? session && { ...session, ...mergeAgentSnapshot(session) } : registry.snapshot[key]),
      projectId: entry.projectId, slot: entry.slot, opening: false, error: message });
    entry.attentions.forEach(receiveAttention);
    entry.completions.forEach(receiveCompletion);
    registry.showError(message);
  }
}

async function showTerminal(entry: Entry, shouldFocus: () => boolean) {
  await initTerminalRegistry(registry.showError);
  // A missing bundled face must not keep the terminal from opening; the fallback fonts still work.
  const fontFamily = await loadTerminalFonts().then(() => TERMINAL_FONT_FAMILY, (error) => {
    console.warn('Terminal font:', error);
    return TERMINAL_FALLBACK_FONT_FAMILY;
  });
  if (entry.disposed || !visible(entry)) return;
  entry.terminal.options.fontFamily = fontFamily;
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
  fitTerminal(entry.projectId, entry.slot, shouldFocus());
  if (!entry.sessionId && !entry.opening) void startHydrate(entry, false);
}

export function attachTerminal(projectId: string, slot: number, host: HTMLDivElement, onReady?: () => void) {
  const entry = registry.entries.get(terminalKey(projectId, slot)) ?? createEntry(projectId, slot);
  host.appendChild(entry.container);
  let detached = false;
  const attached = () => !detached && !entry.disposed && entry.container.parentElement === host;
  let readyFrame = 0;
  const ready = () => {
    if (attached() && onReady) onReady();
  };
  void showTerminal(entry, () => false).then(async () => {
    if (!onReady) return;
    await entry.opening;
    if (!attached() || !entry.opened) return;
    // Let xterm consume the restored writes and draw rows before revealing its layer.
    entry.terminal.write('', () => {
      if (!attached()) return;
      entry.terminal.refresh(0, entry.terminal.rows - 1);
      readyFrame = requestAnimationFrame(() => { readyFrame = requestAnimationFrame(ready); });
    });
  }).catch(error => {
    if (!attached()) return;
    const message = `终端显示失败：${error}`;
    registry.showError(message);
    publish({ ...registry.snapshot[terminalKey(projectId, slot)], projectId, slot, opening: false, error: message });
    readyFrame = requestAnimationFrame(ready);
  });
  const frame = requestAnimationFrame(() => fitTerminal(projectId, slot));
  return () => {
    detached = true;
    cancelAnimationFrame(frame);
    cancelAnimationFrame(readyFrame);
    if (entry.container.parentElement !== host) return;
    if (!entry.opened || entry.disposed) {
      entry.container.remove();
      return;
    }
    // Keep xterm's renderer mounted while its project or slot is hidden.
    if (!registry.inactiveHost) {
      const inactiveHost = document.createElement('div');
      inactiveHost.hidden = true;
      inactiveHost.inert = true;
      document.body.appendChild(inactiveHost);
      registry.inactiveHost = inactiveHost;
    }
    registry.inactiveHost.appendChild(entry.container);
  };
}

export async function restartTerminal(projectId: string, slot: number, sessionId: string) {
  await initTerminalRegistry(registry.showError);
  const key = terminalKey(projectId, slot);
  if (registry.snapshot[key]?.sessionId !== sessionId) return;
  const entry = registry.entries.get(key) ?? createEntry(projectId, slot);
  return startHydrate(entry, true);
}

export function retryTerminal(projectId: string, slot: number) {
  const entry = registry.entries.get(terminalKey(projectId, slot));
  if (entry) void showTerminal(entry, () => true).catch(error => registry.showError(String(error)));
}

export async function createTerminal(projectId: string): Promise<number> {
  await initTerminalRegistry(registry.showError);
  const result = await invoke<TerminalOpenResult>('terminal_create', { projectId, cols: 80, rows: 24 });
  registerTerminalResult(result.session);
  return result.session.slot;
}

export function registerTerminalResult(session: TerminalSessionInfo) {
  const current = registry.snapshot[terminalKey(session.projectId, session.slot)];
  if (!current?.sessionId) receiveOpened(session);
  else if (current.sessionId === session.sessionId && current.hasAgentRecovery !== session.hasAgentRecovery) {
    publish({ ...current, hasAgentRecovery: session.hasAgentRecovery });
  }
}

export async function closeTerminal(projectId: string, slot: number, sessionId: string) {
  await invoke('terminal_close', { projectId, slot, sessionId });
  receiveClosed({ projectId, slot, sessionId });
}
