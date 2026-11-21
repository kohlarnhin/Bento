import { useState } from 'react';
import { Archive, ArchiveRestore, ChevronLeft, Copy, FileText, Pin, PinOff, SquareTerminal, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useHasProjectLogs } from '../lib/useProjectLogs';
import type { ProjectConfig, Snippet } from '../types';
import { SidebarMenu, type SidebarMenuItem } from './SidebarMenu';

export function ProjectActionsMenu({ project, x, y, keyboard, onClose, onOpenLogs, onDelete, sidebar, snippets, onRunSnippet }: {
  project: ProjectConfig;
  x: number;
  y: number;
  keyboard: boolean;
  onClose: () => void;
  onOpenLogs: () => void;
  onDelete: () => void;
  snippets: Snippet[];
  onRunSnippet: (command: string) => void;
  sidebar: {
    onAddTerminal: () => void;
    terminalLimit?: number;
    onTogglePinned: () => void;
    onToggleArchived: () => void;
  };
}) {
  const hasLogs = useHasProjectLogs(project.id);
  const [submenuProject, setSubmenuProject] = useState<string>();
  const submenu = submenuProject === project.id && snippets.length > 0;
  const items: SidebarMenuItem[] = [
    { label: '添加终端', icon: <SquareTerminal />, onSelect: sidebar.onAddTerminal, disabled: sidebar.terminalLimit != null,
      title: sidebar.terminalLimit != null ? `已达上限（${sidebar.terminalLimit} 个），可在设置中调整` : undefined },
    { label: '复制路径', icon: <Copy />, onSelect: () => {
      void navigator.clipboard.writeText(project.path)
        .then(() => toast.success('项目路径已复制'))
        .catch(error => toast.error(String(error)));
    } },
  ];
  if (snippets.length) items.splice(1, 0, {
    label: '运行常用命令 ›', icon: <SquareTerminal />, keepOpen: true, onSelect: () => setSubmenuProject(project.id),
  });
  if (hasLogs) items.push({ label: '查看启动日志', icon: <FileText />, onSelect: onOpenLogs });
  items.push(
    { label: project.pinned ? '取消置顶' : '置顶项目', icon: project.pinned ? <PinOff /> : <Pin />, onSelect: sidebar.onTogglePinned },
    { label: project.archived ? '取消归档' : '归档项目', icon: project.archived ? <ArchiveRestore /> : <Archive />, onSelect: sidebar.onToggleArchived },
    { label: '删除项目…', icon: <Trash2 />, destructive: true, separatorBefore: true, onSelect: onDelete },
  );
  const commands = [
    { label: '返回项目操作', keepOpen: true, icon: <ChevronLeft />, onSelect: () => setSubmenuProject(undefined) },
    ...snippets.map(snippet => ({ id: snippet.id, label: snippet.name || snippet.command, title: snippet.command, onSelect: () => onRunSnippet(snippet.command) })),
  ];
  return <SidebarMenu x={x} y={y} items={submenu ? commands : items} onClose={onClose} className="project-actions-menu" projectContext keyboard={keyboard}
    label={`${project.displayName || project.name} 的${submenu ? '常用命令' : '更多操作'}`} />;
}
