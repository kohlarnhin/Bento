import type { TerminalAgent } from '../types';

export const MIN_TERMINAL_FONT_SIZE = 12;
export const MAX_TERMINAL_FONT_SIZE = 20;

export function clampTerminalFontSize(size: number): number {
  return Math.max(MIN_TERMINAL_FONT_SIZE, Math.min(MAX_TERMINAL_FONT_SIZE, size));
}

export function normalizeTerminalTitle(raw: string): string {
  const title = raw.replace(/[\r\n\t]/g, ' ').replace(/[\p{Cc}\p{Cf}]/gu, '')
    .replace(/\s+/gu, ' ').trim().replace(/^[^\p{L}\p{N}~/]+/u, '');
  // Check before stripping punctuation so a shell's path cannot become a title.
  const path = title.replace(/^[^\s:@]+@[^\s:]+:/u, '');
  if (/^[~/]/u.test(path)) return '';
  return [...title.replace(/^[^\p{L}\p{N}]+/u, '')].slice(0, 80).join('');
}

// Fixed trailing windows: continuous changes cannot starve the final title.
export function createTitlePublisher(publish: (title: string) => void) {
  let published = '';
  let pending = '';
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    update(raw: string) {
      pending = normalizeTerminalTitle(raw);
      if (pending === published) {
        clearTimeout(timer);
        timer = undefined;
      } else if (timer === undefined) {
        timer = setTimeout(() => {
          timer = undefined;
          published = pending;
          publish(published);
        }, 500);
      }
    },
    reset() {
      clearTimeout(timer);
      timer = undefined;
      published = pending = '';
    },
  };
}

interface AttentionTarget {
  projectId: string;
  slot: number;
  attention?: boolean;
  attentionAt?: number;
  agent?: TerminalAgent | null;
}

export function terminalsNeedingAttention<T extends AttentionTarget>(terminals: T[]): T[] {
  return terminals.filter(terminal => terminal.attention).sort((a, b) =>
    (b.attentionAt ?? 0) - (a.attentionAt ?? 0)
      || a.projectId.localeCompare(b.projectId) || a.slot - b.slot,
  );
}

export const agentNames = { claude: 'Claude', codex: 'Codex' } as const;
export const agentStateLabels = { waiting: '等你确认', done: '完成', thinking: '思考中' } as const;

function agentPriority(agent: TerminalAgent): number {
  if (agent.state === 'waiting') return 0;
  if (agent.state === 'thinking') return 2;
  return agent.acknowledged ? 3 : 1;
}

export function agentNeedsAttention(agent: TerminalAgent | null | undefined): boolean {
  return !!agent && agentPriority(agent) < 2;
}

type AgentTarget = AttentionTarget & { agent: TerminalAgent };
const hasAgent = <T extends AttentionTarget>(terminal: T): terminal is T & AgentTarget => !!terminal.agent;
const compareAgents = (a: AgentTarget, b: AgentTarget) =>
  agentPriority(a.agent) - agentPriority(b.agent) || a.agent.stateStartedAt - b.agent.stateStartedAt
    || a.projectId.localeCompare(b.projectId) || a.slot - b.slot;

export interface ProjectAgent {
  agent: TerminalAgent;
  slot: number;
  multiple: boolean;
}

export function agentsByProject(terminals: readonly AttentionTarget[]): Map<string, ProjectAgent> {
  const projects = new Map<string, ProjectAgent>();
  const groups = new Map<string, AgentTarget[]>();
  for (const terminal of terminals.filter(hasAgent)) {
    const group = groups.get(terminal.projectId);
    if (group) group.push(terminal);
    else groups.set(terminal.projectId, [terminal]);
  }
  for (const [id, group] of groups) {
    const { agent, slot } = group.sort(compareAgents)[0];
    if (agentPriority(agent) < 3) projects.set(id, { agent, slot, multiple: group.length > 1 });
  }
  return projects;
}

export function nextAttentionTarget<T extends AttentionTarget>(terminals: readonly T[], current?: { projectId: string; slot: number }): T | undefined {
  const waiting = terminals.filter(hasAgent).filter(terminal => agentNeedsAttention(terminal.agent)).sort(compareAgents);
  const targets = [...waiting, ...terminalsNeedingAttention(terminals.filter(terminal => !terminal.agent))];
  if (!current) return targets[0];
  const index = targets.findIndex(terminal => terminal.projectId === current.projectId && terminal.slot === current.slot);
  return targets[index + 1] ?? (index > 0 ? targets[0] : undefined);
}

export function agentMinutesSince(start: number, now: number): number {
  return Math.max(0, Math.floor((now - start) / 60_000));
}

export function formatAgentDuration(elapsedMs: number): { compact: string; full: string } {
  const seconds = Math.max(0, Math.floor(elapsedMs / 1_000));
  if (seconds < 60) return { compact: `${seconds}s`, full: `${seconds} 秒` };

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return { compact: `${minutes}m`, full: `${minutes} 分钟` };

  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return { compact: `${hours}h ${remainingMinutes}m`, full: `${hours} 小时 ${remainingMinutes} 分钟` };
}

export function agentDurationRefreshDelay(elapsedMs: number): number {
  const elapsed = Math.max(0, elapsedMs);
  const step = elapsed < 60_000 ? 1_000 : 60_000;
  return step - elapsed % step + Math.max(0, -elapsedMs);
}

export function firstOpenedTerminalSlot(projectId: string, terminals: readonly {
  projectId: string;
  slot: number;
  sessionId?: string;
}[]): number | undefined {
  let first: number | undefined;
  for (const terminal of terminals) {
    if (terminal.projectId === projectId && terminal.sessionId && (first === undefined || terminal.slot < first)) {
      first = terminal.slot;
    }
  }
  return first;
}
