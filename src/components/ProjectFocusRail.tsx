import type { ProjectConfig, ProjectStatus } from '../types';
import { ProjectAvatar } from './ProjectAvatar';

export function ProjectFocusRail({ projects, selectedId, statuses, onSelect }: {
  projects: ProjectConfig[];
  selectedId: string;
  statuses: Record<string, ProjectStatus>;
  onSelect: (projectId: string) => void;
}) {
  return <aside className="terminal-focus-rail" aria-label="专注模式项目栏" data-terminal-focus-rail>
    <div className="terminal-focus-rail-chrome" data-tauri-drag-region aria-hidden="true" />
    <nav aria-label="项目">
      {projects.map(project => {
        const name = project.displayName || project.name;
        const status = statuses[project.id] || 'stopped';
        return <button key={project.id} type="button" className="terminal-focus-project" title={name} aria-label={name}
          aria-current={project.id === selectedId ? 'page' : undefined} data-focus-project={project.id} onClick={() => onSelect(project.id)}>
          <ProjectAvatar project={project} status={status} />
          {status !== 'stopped' && <span className="h1-status-dot" data-tone={status} aria-hidden="true" />}
        </button>;
      })}
    </nav>
  </aside>;
}
