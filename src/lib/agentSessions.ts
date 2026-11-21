import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useMemo, useState } from 'react';
import type { TerminalAgentKind, TerminalSessionInfo } from '../types';
import type { TerminalStatus } from './terminalRegistry';

export interface AgentHistorySession {
  tool: TerminalAgentKind;
  id: string;
  cwd: string;
  title: string;
  startedAt: number;
  lastAt: number;
  durationMs: number;
  fileCount: number | null;
}

export type AgentArgv = ['claude' | 'codex'] | ['claude', '--resume', string] | ['codex', 'resume', string];
export type AgentRecoveryTarget = { slot: number; sessionId: string };
export type OpenAgent = (argv: AgentArgv, recovery?: AgentRecoveryTarget) => Promise<void>;
export type AgentRecoveries = Record<string, AgentHistorySession>;

export interface AgentMenuData {
  tools: TerminalAgentKind[];
  sessions: AgentHistorySession[];
}

export function agentArgv(tool: TerminalAgentKind, id?: string): AgentArgv {
  if (id === undefined) return [tool];
  return tool === 'claude' ? ['claude', '--resume', id] : ['codex', 'resume', id];
}

export async function activateAgentTerminal(projectId: string, argv: AgentArgv,
  select: (terminal: TerminalSessionInfo) => void, recovery?: AgentRecoveryTarget) {
  const terminal = await invoke<TerminalSessionInfo>('terminal_agent_start', { projectId, argv, recovery });
  select(terminal);
}

const clock = new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
const calendar = new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric' });

export function agentSessionDate(timestamp: number, now = new Date()) {
  const date = new Date(timestamp);
  if (date.toDateString() === now.toDateString()) return `今天 ${clock.format(date)}`;
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  return date.toDateString() === yesterday.toDateString() ? `昨天 ${clock.format(date)}` : calendar.format(date);
}

export function useAgentRecoveries(terminals: Record<string, TerminalStatus>) {
  const batch = useMemo(() => Object.values(terminals).filter(terminal => terminal.hasAgentRecovery)
    .map(terminal => terminal.sessionId).sort().join(','), [terminals]);
  const [resolved, setResolved] = useState<AgentRecoveries>({});
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const dismiss = useCallback((sessionId: string) => setDismissed(previous => new Set(previous).add(sessionId)), []);
  useEffect(() => {
    if (!batch) return;
    let current = true;
    void invoke<AgentRecoveries>('terminal_agent_recoveries').then(result => {
      if (current) setResolved(result);
    }).catch(() => {});
    return () => { current = false; };
  }, [batch]);

  return useMemo(() => {
    const recoveries: AgentRecoveries = {};
    const counts = new Map<string, number>();
    for (const { projectId, sessionId, alive, agent, hasAgentRecovery } of Object.values(terminals)) {
      if (!sessionId || !alive || agent || !hasAgentRecovery || dismissed.has(sessionId) || !resolved[sessionId]) continue;
      recoveries[sessionId] = resolved[sessionId];
      counts.set(projectId, (counts.get(projectId) ?? 0) + 1);
    }
    return { recoveries, counts, dismiss };
  }, [terminals, resolved, dismissed, dismiss]);
}
