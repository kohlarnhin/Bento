import { projectAvatar } from '../lib/sidebarModel';
import { cn } from '../lib/utils';
import type { ProjectConfig, ProjectStatus } from '../types';

export function ProjectAvatar({ project, status = 'stopped', conflict = false, className }: {
  project: ProjectConfig; status?: ProjectStatus; conflict?: boolean; className?: string;
}) {
  const avatar = projectAvatar(project.name);
  const label = conflict ? '端口冲突' : { running: '运行中', starting: '启动中', error: '运行异常', stopped: '未运行' }[status];
  return <span className={cn('project-avatar relative grid shrink-0 place-items-center font-bold text-white', className)}
    style={{ backgroundColor: avatar.color }} aria-label={label} title={label} data-project-avatar={project.id}>
    {avatar.initial}
    {(status !== 'stopped' || conflict) && <span className={cn('absolute bottom-[var(--project-avatar-dot-offset,-2px)] right-[var(--project-avatar-dot-offset,-2px)] size-[9px] rounded-full border-2 border-white',
      conflict || status === 'starting' ? 'bg-warning' : status === 'error' ? 'bg-destructive' : 'bg-[#10b981]', status === 'starting' && 'animate-pulse')} />}
  </span>;
}
