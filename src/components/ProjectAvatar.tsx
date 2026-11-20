import { projectInitial } from '../lib/sidebarModel';
import { cn } from '../lib/utils';
import type { ProjectConfig, ProjectStatus } from '../types';

export function ProjectAvatar({ project, status = 'stopped', conflict = false, className }: {
  project: ProjectConfig; status?: ProjectStatus; conflict?: boolean; className?: string;
}) {
  const label = conflict ? '端口冲突' : { running: '运行中', starting: '启动中', error: '运行异常', stopped: '未运行' }[status];
  return <span className={cn('project-avatar grid shrink-0 place-items-center font-semibold', className)}
    aria-label={label} title={label} data-project-avatar={project.id}>
    {projectInitial(project.displayName || project.name)}
  </span>;
}
