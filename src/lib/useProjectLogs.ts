import { useCallback, useEffect, useRef, useState } from 'react';
import type { LogEntry } from '../types';

type IncomingEntry = Pick<LogEntry, 'text' | 'stream'>;
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

export function useProjectLogs() {
  const [logs, setLogs] = useState<Record<string, LogEntry[]>>({});
  const pending = useRef<Record<string, LogEntry[]>>({});
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const nextId = useRef(0);

  const flush = useCallback(() => {
    clearTimeout(timer.current);
    timer.current = undefined;
    const batches = pending.current;
    pending.current = {};
    if (!Object.keys(batches).length) return;
    if (debugMetrics) debugMetrics.stateFlushes++;
    setLogs(previous => {
      const next = { ...previous };
      for (const [projectId, entries] of Object.entries(batches)) {
        next[projectId] = [...(previous[projectId] || []), ...entries].slice(-800);
      }
      return next;
    });
  }, []);

  const enqueue = useCallback((projectId: string, entries: IncomingEntry[]) => {
    if (!entries.length) return;
    const timestamp = Date.now();
    const incoming = entries.map(entry => ({ ...entry, projectId, timestamp, id: String(++nextId.current) }));
    pending.current[projectId] = [...(pending.current[projectId] || []), ...incoming].slice(-800);
    if (debugMetrics) debugMetrics.linesReceived += entries.length;
    if (timer.current === undefined) timer.current = setTimeout(flush, 100);
  }, [flush]);

  const clear = useCallback((projectId: string, remove = false) => {
    delete pending.current[projectId];
    setLogs(previous => {
      const next = { ...previous };
      if (remove) delete next[projectId];
      else next[projectId] = [];
      return next;
    });
  }, []);

  useEffect(() => () => {
    clearTimeout(timer.current);
    timer.current = undefined;
    pending.current = {};
  }, []);

  return { logs, enqueue, flush, clear };
}
