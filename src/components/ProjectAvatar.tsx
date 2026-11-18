import { projectAvatar } from '../lib/sidebarModel';
import { cn } from '../lib/utils';
import type { ProjectConfig, ProjectStatus } from '../types';

export function ProjectAvatar({ project, status = 'stopped', conflict = false, className }: {
  project: ProjectConfig; status?: ProjectStatus; conflict?: boolean; className?: string;
}) {
  const avatar = projectAvatar(project.name);
  const label = conflict ? '端口冲突' : { running: '运行中', starting: '启动中', error: '运行异常', stopped: '未运行' }[status];
  return <span className={cn('relative grid size-5 shrink-0 place-items-center rounded-[5px] text-[10px] font-semibold text-white', className)}
    style={{ backgroundColor: avatar.color }} aria-label={label} title={label} data-project-avatar={project.id}>
    {avatar.initial}
    {(status !== 'stopped' || conflict) && <span className={cn('absolute -bottom-0.5 -right-0.5 size-2 rounded-full border-2 border-sidebar',
      conflict || status === 'starting' ? 'bg-warning' : status === 'error' ? 'bg-destructive' : 'bg-success', status === 'starting' && 'animate-pulse')} />}
  </span>;
}
