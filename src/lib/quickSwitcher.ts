import type { ProjectConfig } from '../types';
import { agentArgv, type AgentArgv, type ProjectAgentSession } from './agentSessions.ts';
import { nameMatch } from './sidebarModel.ts';

export function quickSwitcherSessionOptions(projects: readonly ProjectConfig[], sessions: readonly ProjectAgentSession[],
  query: string, onOpenAgent: (projectId: string, argv: AgentArgv) => void) {
  if (!query.trim()) return [];
  const projectsById = new Map(projects.map(project => [project.id, project]));
  return sessions.flatMap(session => {
    const project = projectsById.get(session.projectId);
    const match = project && nameMatch(session.title, query);
    return project && match ? [{ project, session, indices: match.indices }] : [];
  }).sort((a, b) => b.session.lastAt - a.session.lastAt).slice(0, 8).map(option => ({
    ...option,
    id: `session:${option.project.id}:${option.session.tool}:${option.session.id}`,
    run: () => onOpenAgent(option.project.id, agentArgv(option.session.tool, option.session.id)),
  }));
}

export function quickSwitcherResultKey(key: 'ArrowUp' | 'ArrowDown' | 'Enter', repeat: boolean, activeIndex: number,
  options: readonly { run: () => void }[]) {
  if (key === 'Enter') {
    if (!repeat) options[activeIndex]?.run();
    return;
  }
  const count = options.length;
  if (count) return (activeIndex + (key === 'ArrowDown' ? 1 : -1) + count) % count;
}
