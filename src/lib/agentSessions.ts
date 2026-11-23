import { invoke } from '@tauri-apps/api/core';
import type { AgentSessionTool, TerminalSessionInfo } from '../types';

export interface AgentHistorySession {
  tool: AgentSessionTool;
  id: string;
  cwd: string;
  title: string;
  startedAt: number;
  lastAt: number;
  durationMs: number;
  fileCount: number | null;
}

export interface ProjectAgentSession extends AgentHistorySession {
  projectId: string;
}

export type AgentArgv = ['claude' | 'codex'] | ['claude', '--resume', string] | ['codex', 'resume', string];
export type OpenAgent = (argv: AgentArgv) => Promise<void>;

export interface AgentMenuData {
  tools: AgentSessionTool[];
  sessions: AgentHistorySession[];
}

export async function loadAgentIndex<T>(command: 'terminal_agent_project_sessions' | 'terminal_agent_menu',
  args: Record<string, string>, receive: (data: T) => void, cachedOnly = false) {
  try {
    receive(await invoke<T>(command, { ...args, cached: true }));
  } catch (error) {
    if (cachedOnly) throw error;
  }
  if (!cachedOnly) receive(await invoke<T>(command, args));
}

export function agentArgv(tool: AgentSessionTool, id?: string): AgentArgv {
  if (id === undefined) return [tool];
  return tool === 'claude' ? ['claude', '--resume', id] : ['codex', 'resume', id];
}

export async function activateAgentTerminal(projectId: string, argv: AgentArgv,
  select: (terminal: TerminalSessionInfo) => void, prepare: (terminal: TerminalSessionInfo) => Promise<void>) {
  const { session, launchId } = await invoke<{ session: TerminalSessionInfo; launchId: string | null }>(
    'terminal_agent_start', { projectId, argv });
  select(session);
  if (!launchId) return session;
  await prepare(session);
  return invoke<TerminalSessionInfo>('terminal_agent_launch', {
    projectId: session.projectId, slot: session.slot, sessionId: session.sessionId, launchId,
  });
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
