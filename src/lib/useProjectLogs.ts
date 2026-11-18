import { useCallback, useSyncExternalStore } from 'react';
import type { LogEntry } from '../types';
import { DASHBOARD_LOG_LIMIT } from './dashboard';

type IncomingEntry = Pick<LogEntry, 'text' | 'stream'>;
interface LogStore {
  lines: Map<string, LogEntry[]>;
  pending: Map<string, LogEntry[]>;
  subscribers: Map<string, Set<() => void>>;
  timer?: ReturnType<typeof setTimeout>;
  nextId: number;
}

const store: LogStore = import.meta.hot?.data.projectLogs ?? {
  lines: new Map(), pending: new Map(), subscribers: new Map(), nextId: 0,
};
if (import.meta.hot) import.meta.hot.data.projectLogs = store;
const emptyLines: LogEntry[] = [];
const debugMetrics = import.meta.env.DEV ? { stateFlushes: 0, linesReceived: 0 } : undefined;

// Read by the real-webview harness; the counters are absent in production.
export function logDeliveryMetrics(reset = false) {
  const snapshot = debugMetrics && { ...debugMetrics };
  if (reset && debugMetrics) {
    debugMetrics.stateFlushes = 0;
    debugMetrics.linesReceived = 0;
  }
  return snapshot;
}

function notify(projectId: string) {
  store.subscribers.get(projectId)?.forEach(subscriber => subscriber());
}

function flush() {
  clearTimeout(store.timer);
  store.timer = undefined;
  const batches = store.pending;
  store.pending = new Map();
  if (!batches.size) return;
  if (debugMetrics) debugMetrics.stateFlushes++;
  for (const [projectId, entries] of batches) {
    store.lines.set(projectId, [...(store.lines.get(projectId) ?? emptyLines), ...entries].slice(-DASHBOARD_LOG_LIMIT));
    notify(projectId);
  }
}

function enqueue(projectId: string, entries: IncomingEntry[]) {
  if (!entries.length) return;
  const timestamp = Date.now();
  const incoming = entries.map(entry => ({ ...entry, projectId, timestamp, id: String(++store.nextId) }));
  store.pending.set(projectId, [...(store.pending.get(projectId) ?? emptyLines), ...incoming].slice(-DASHBOARD_LOG_LIMIT));
  if (debugMetrics) debugMetrics.linesReceived += entries.length;
  if (store.timer === undefined) store.timer = setTimeout(flush, 100);
}

function clear(projectId: string, remove = false) {
  store.pending.delete(projectId);
  if (remove) store.lines.delete(projectId);
  else store.lines.set(projectId, emptyLines);
  notify(projectId);
}

function subscribe(projectId: string, subscriber: () => void) {
  let subscribers = store.subscribers.get(projectId);
  if (!subscribers) store.subscribers.set(projectId, subscribers = new Set());
  subscribers.add(subscriber);
  return () => {
    subscribers.delete(subscriber);
    if (!subscribers.size) store.subscribers.delete(projectId);
  };
}

export const projectLogs = { enqueue, flush, clear };

export function useHasProjectLogs(projectId: string) {
  return useSyncExternalStore(
    useCallback(subscriber => subscribe(projectId, subscriber), [projectId]),
    useCallback(() => (store.lines.get(projectId)?.length ?? 0) > 0, [projectId]),
  );
}

export function useProjectLogLines(projectId: string) {
  return useSyncExternalStore(
    useCallback(subscriber => subscribe(projectId, subscriber), [projectId]),
    useCallback(() => store.lines.get(projectId) ?? emptyLines, [projectId]),
  );
}
