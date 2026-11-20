import { useMemo } from 'react';
import { Settings } from 'lucide-react';
import type { ProjectConfig, ProjectGroup, ProjectStatus } from '../types';
import { focusRailSections } from '../lib/sidebarModel';
import { ProjectAvatar } from './ProjectAvatar';

export function ProjectFocusRail({ projects, groups, active, selectedId, statuses, onSelect, onOpenSettings }: {
  projects: ProjectConfig[];
  groups: ProjectGroup[];
  active: boolean;
  selectedId?: string;
  statuses: Record<string, ProjectStatus>;
  onSelect: (projectId: string) => void;
  onOpenSettings: () => void;
}) {
  const sections = useMemo(() => focusRailSections(projects, groups, statuses, selectedId), [projects, groups, statuses, selectedId]);
  return <aside className="terminal-focus-rail" aria-label="专注模式项目栏" data-terminal-focus-rail {...{ inert: active ? undefined : '' }}>
    <div className="terminal-focus-rail-chrome" data-tauri-drag-region aria-hidden="true" />
    <nav aria-label="项目">
      {sections.map(section => <div key={section.id} className="terminal-focus-group" role="group" aria-label={section.name}>
        {section.projects.map(project => {
          const name = project.displayName || project.name;
          const status = statuses[project.id] || 'stopped';
          return <button key={project.id} type="button" className="terminal-focus-project" title={name} aria-label={name}
            aria-current={project.id === selectedId ? 'page' : undefined} data-focus-project={project.id} onClick={() => onSelect(project.id)}>
            <ProjectAvatar project={project} status={status} />
            {status !== 'stopped' && <span className="h1-status-dot" data-tone={status} aria-hidden="true" />}
          </button>;
        })}
      </div>)}
    </nav>
    <button type="button" className="terminal-focus-settings" title="设置" aria-label="设置" data-focus-settings onClick={onOpenSettings}>
      <Settings size={15} aria-hidden="true" />
    </button>
  </aside>;
}
