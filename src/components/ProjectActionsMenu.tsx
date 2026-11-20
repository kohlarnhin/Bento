import { toast } from 'sonner';
import { useHasProjectLogs } from '../lib/useProjectLogs';
import type { ProjectConfig } from '../types';
import { SidebarMenu, type SidebarMenuItem } from './SidebarMenu';

export function ProjectActionsMenu({ project, x, y, onClose, onOpenLogs, onDelete }: {
  project: ProjectConfig;
  x: number;
  y: number;
  onClose: () => void;
  onOpenLogs: () => void;
  onDelete: () => void;
}) {
  const hasLogs = useHasProjectLogs(project.id);
  const items: SidebarMenuItem[] = [{
    label: '复制路径',
    onSelect: () => {
      void navigator.clipboard.writeText(project.path)
        .then(() => toast.success('项目路径已复制'))
        .catch(error => toast.error(String(error)));
    },
  }];
  if (hasLogs) items.push({ label: '查看启动日志', onSelect: onOpenLogs });
  items.push({ label: '删除项目…', destructive: true, separatorBefore: true, onSelect: onDelete });
  return <SidebarMenu x={x} y={y} items={items} onClose={onClose} className="project-actions-menu"
    label={`${project.displayName || project.name} 的更多操作`} />;
}
