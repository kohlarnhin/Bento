import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { PointerEvent as ReactPointerEvent, RefObject } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { Archive, ArchiveRestore, ArrowDownToLine, ChevronRight, FolderOpen, FolderPlus, GripVertical, LayoutDashboard, MoreHorizontal, Pencil, Pin, PinOff, Plus, RefreshCw, RotateCcw, Search, Settings, Terminal, Trash2, X } from 'lucide-react';
import { toast } from 'sonner';
import type { PortConflict, ProjectConfig, ProjectGroup, ProjectSelection, ProjectStatus } from '../types';
import { groupColor, matchesSidebarFilter, moveProjectOrder, sidebarDensity, sidebarSections, type SidebarFilter, type SidebarSection } from '../lib/sidebarModel';
import { projectPorts } from '../lib/projectServices';
import { closeTerminal, getProjectTerminalSlots, getTerminalSnapshot, restartTerminal, subscribeTerminals, terminalKey, type TerminalStatus } from '../lib/terminalRegistry';
import { cn } from '../lib/utils';
import { recordRender } from '../lib/renderMetrics';
import logo from '../assets/logo.svg';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';
import { ScrollArea } from './ui/scroll-area';
import { SegmentedControl } from './ui/segmented-control';
import { Input } from './ui/input';
import { ProjectAvatar } from './ProjectAvatar';
import { SidebarMenu, type SidebarMenuItem } from './SidebarMenu';

interface Props {
  projects: ProjectConfig[];
  folderDropName: string | null;
  groups: ProjectGroup[];
  filter: SidebarFilter;
  onFilterChange: (filter: SidebarFilter) => void;
  attentionProjects: ReadonlySet<string>;
  changedProjects: ReadonlySet<string>;
  onQuickSwitch: () => void;
  onUpdateGroup: (id: string, changes: { name?: string; collapsed?: boolean }) => Promise<void>;
  onReorderGroups: (ids: string[]) => Promise<void>;
  onGroupAction: (action: 'create' | 'rename' | 'delete', group?: ProjectGroup) => void;
  onProjectFlags: (id: string, flags: { pinned?: boolean; archived?: boolean }) => Promise<void>;
  visibleProjects: ProjectConfig[];
  searchQuery: string;
  searchInputRef: RefObject<HTMLInputElement>;
  onSearchChange: (query: string) => void;
  onReorder: (ids: string[], move?: { id: string; groupId: string | null }) => Promise<void>;
  reordering: boolean;
  statuses: Record<string, ProjectStatus>;
  conflicts: PortConflict[];
  selection: ProjectSelection | null;
  activeSlots: Record<string, number>;
  maxPerProject: number;
  loading: boolean;
  projectsLoaded: boolean;
  onSelect: (selection: ProjectSelection) => void;
  onSelectTerminal: (projectId: string, slot: number) => void;
  onCreateTerminal: (projectId: string) => Promise<void>;
  onRefresh: () => void;
  onCreate: () => void;
  managedCount: number;
  onStopAll: () => void;
  currentVersion: string;
  availableVersion?: string;
  onCheckUpdates: () => void;
  onOpenSettings: () => void;
}

interface ProjectDrag {
  id: string;
  kind: 'project' | 'group';
  ids: string[];
  pointerId: number;
  button: HTMLButtonElement;
  startX: number;
  startY: number;
  x: number;
  y: number;
  offsetX: number;
  offsetY: number;
  active: boolean;
  valid: boolean;
  beforeId: string | null;
  afterId: string | null;
  targetGroupId: string | null;
  ghost?: HTMLElement;
  frame?: number;
}

const RUNNING_COLLAPSED_KEY = 'bento.sidebar.runningCollapsed';
const ARCHIVED_COLLAPSED_KEY = 'bento.sidebar.archivedCollapsed';
const nativeVibrancy = isTauri() && navigator.platform.startsWith('Mac');

function readRunningCollapsed(): boolean {
  try { return JSON.parse(localStorage.getItem(RUNNING_COLLAPSED_KEY) ?? 'false') === true; }
  catch { return false; }
}

function GroupHeadingMarker({ color, expanded }: { color: string; expanded?: boolean }) {
  return <span aria-hidden="true" className="relative size-[7px] shrink-0">
    {expanded !== undefined && <ChevronRight className={cn('sidebar-heading-disclosure absolute left-1/2 top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 motion-safe:transition-transform', expanded && 'rotate-90')} />}
    <span className="sidebar-heading-dot block size-full rounded-full" style={{ backgroundColor: color }} />
  </span>;
}

function RestartTerminalButton({ projectId, slot, status, active, onSelect }: {
  projectId: string; slot: number; status?: TerminalStatus; active: boolean; onSelect: Props['onSelectTerminal'];
}) {
  const [confirmUntil, setConfirmUntil] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    setConfirmUntil(0);
    return () => clearTimeout(timer.current);
  }, [status?.sessionId]);
  const confirming = !!status?.alive && confirmUntil > 0;
  const label = confirming ? '再次点击确认重启' : `重启终端 ${slot}`;
  return (
    <ActionTooltip label={label} side="right">
      <Button
        size="icon-sm"
        variant={confirming ? 'ghost-destructive' : 'ghost'}
        className={cn('sidebar-terminal-action no-drag shrink-0 opacity-0 transition-opacity group-hover/terminal:opacity-100 group-focus-within/terminal:opacity-100', (active || confirming) && 'opacity-100')}
        aria-label={label}
        title={label}
        data-sidebar-terminal-restart={projectId}
        data-sidebar-terminal-slot={slot}
        data-confirming={confirming}
        disabled={status?.opening}
        onClick={() => {
          onSelect(projectId, slot);
          if (status?.alive && Date.now() >= confirmUntil) {
            setConfirmUntil(Date.now() + 3000);
            clearTimeout(timer.current);
            timer.current = setTimeout(() => setConfirmUntil(0), 3000);
            return;
          }
          clearTimeout(timer.current);
          setConfirmUntil(0);
          void restartTerminal(projectId, slot);
        }}
      >
        <RotateCcw data-icon="inline-start" className={cn(status?.opening && 'animate-spin')} />
      </Button>
    </ActionTooltip>
  );
}

function CloseTerminalButton({ projectId, slot, status, active }: {
  projectId: string; slot: number; status?: TerminalStatus; active: boolean;
}) {
  const [confirmUntil, setConfirmUntil] = useState(0);
  const [closing, setClosing] = useState(false);
  const pending = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    setConfirmUntil(0);
    return () => clearTimeout(timer.current);
  }, [status?.sessionId]);
  const confirming = !!status?.alive && confirmUntil > 0;
  const label = confirming ? '再次点击确认关闭' : `关闭终端 ${slot}`;
  return <ActionTooltip label={label} side="right">
    <Button variant={confirming ? 'ghost-destructive' : 'ghost'} size="icon-sm"
      className={cn('sidebar-terminal-action no-drag shrink-0 opacity-0 transition-opacity group-hover/terminal:opacity-100 group-focus-within/terminal:opacity-100', (active || confirming) && 'opacity-100')}
      aria-label={label} title={label} data-sidebar-terminal-close={projectId} data-sidebar-terminal-slot={slot} data-confirming={confirming} disabled={closing || status?.opening}
      onClick={() => {
        if (pending.current) return;
        if (status?.alive && Date.now() >= confirmUntil) {
          setConfirmUntil(Date.now() + 3000);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setConfirmUntil(0), 3000);
          return;
        }
        clearTimeout(timer.current);
        setConfirmUntil(0);
        pending.current = true;
        setClosing(true);
        void closeTerminal(projectId, slot).catch(error => toast.error(`关闭终端失败：${error}`)).finally(() => {
          pending.current = false;
          setClosing(false);
        });
      }}><X /></Button>
  </ActionTooltip>;
}

function NewTerminalButton({ projectId, atLimit, maxPerProject, onCreate }: {
  projectId: string; atLimit: boolean; maxPerProject: number; onCreate: () => Promise<void>;
}) {
  const [creating, setCreating] = useState(false);
  const label = atLimit ? `已达上限（${maxPerProject} 个），可在设置中调整` : '新建终端 (⌘T)';
  return <ActionTooltip label={label} side="right">
    <span className="no-drag mr-1 inline-flex shrink-0 opacity-0 transition-opacity group-hover/project:opacity-100 group-focus-within/project:opacity-100" title={label}>
      <Button variant="ghost" size="icon-sm" aria-label={label} data-sidebar-terminal-new={projectId} disabled={atLimit || creating}
        onClick={() => {
          setCreating(true);
          void onCreate().finally(() => setCreating(false));
        }}><Plus /></Button>
    </span>
  </ActionTooltip>;
}

function TerminalStatusDot({ projectId, slot, status }: { projectId: string; slot: number; status?: TerminalStatus }) {
  if (!status?.attention && !status?.alive) return null;
  const label = status.attention ? '终端有新动静' : '终端会话已打开';
  return <span className={cn('ml-auto size-1.5 shrink-0 rounded-full', status.attention ? 'bg-attention' : 'bg-[var(--sidebar-port)]')}
    aria-label={label} title={label} data-terminal-attention={status.attention ? projectId : undefined} data-sidebar-terminal-slot={slot} />;
}

function TerminalRowLabel({ slot, title }: { slot: number; title?: string }) {
  return <span className={cn('flex min-w-0 flex-1 items-baseline', title ? 'gap-1' : 'gap-2')}>
    <span className="shrink-0">终端 {slot}</span>
    {title && <span className="min-w-0 truncate text-label font-normal text-muted-foreground">
      <span aria-hidden="true">· </span><span data-terminal-title>{title}</span>
    </span>}
  </span>;
}

function SidebarPort({ project, hideOnHover }: { project: ProjectConfig; hideOnHover?: boolean }) {
  const extra = projectPorts(project).filter(port => port !== project.port).length;
  if (project.port == null && !extra) return null;
  return <span className={cn('sidebar-number sidebar-port', hideOnHover && 'group-hover/project:opacity-0 group-focus-within/project:opacity-0')}>
    {project.port ?? '—'}{extra > 0 && <small title={`另有 ${extra} 个服务端口`}>+{extra}</small>}
  </span>;
}

export const ProjectSidebar = memo(function ProjectSidebar({ projects, folderDropName, groups: projectGroups, filter, onFilterChange, attentionProjects, changedProjects, onQuickSwitch, onUpdateGroup, onReorderGroups, onGroupAction, onProjectFlags, visibleProjects, searchQuery, searchInputRef, onSearchChange, onReorder, reordering, statuses, conflicts, selection, activeSlots, maxPerProject, loading, projectsLoaded, onSelect, onSelectTerminal, onCreateTerminal, onRefresh, onCreate, managedCount, onStopAll, currentVersion, availableVersion, onCheckUpdates, onOpenSettings }: Props) {
  recordRender('projectSidebar');
  const [runningCollapsed, setRunningCollapsed] = useState(readRunningCollapsed);
  const [archivedCollapsed, setArchivedCollapsed] = useState(() => {
    try { return JSON.parse(localStorage.getItem(ARCHIVED_COLLAPSED_KEY) ?? 'true') !== false; } catch { return true; }
  });
  const [temporaryCollapsed, setTemporaryCollapsed] = useState(new Set<string>());
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [searchFocused, setSearchFocused] = useState(false);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: SidebarMenuItem[] } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const buckets = useRef(new Map<string, HTMLDivElement>());
  const scrollArea = useRef<HTMLDivElement>(null);
  const runningSection = useRef<HTMLElement>(null);
  const indicator = useRef<HTMLDivElement>(null);
  const drag = useRef<ProjectDrag | null>(null);
  const suppressClick = useRef(false);
  const searching = searchQuery.trim().length > 0;
  const filtering = searching || filter !== 'all';
  const canReorder = !filtering && !loading && !reordering;
  const highlighted = visibleProjects.find(project => project.id === highlightedId)?.id || visibleProjects[0]?.id;
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const visibleIds = useMemo(() => new Set(visibleProjects.map(project => project.id)), [visibleProjects]);
  const sections = useMemo(() => sidebarSections(projects, projectGroups), [projects, projectGroups]);
  const density = sidebarDensity(projects);
  const orderedGroups = [...projectGroups].sort((a, b) => a.sortOrder - b.sortOrder || a.id.localeCompare(b.id));
  const runningProjects = projects.filter(project => matchesSidebarFilter(project, 'running', statuses, attentionProjects, changedProjects));
  const running = runningProjects.length;
  const runningIds = new Set(runningProjects.map(project => project.id));
  const ungrouped = sections.find(section => section.id === 'ungrouped')?.projects ?? [];
  const runningUngrouped = !filtering && !runningCollapsed ? ungrouped.filter(project => runningIds.has(project.id)) : [];
  const runningUngroupedIds = new Set(runningUngrouped.map(project => project.id));
  const versionLabel = currentVersion ? `v${currentVersion}` : '检查更新';
  const updateLabel = availableVersion ? `更新到 v${availableVersion}` : '检查更新';
  useEffect(() => {
    try { localStorage.setItem(RUNNING_COLLAPSED_KEY, JSON.stringify(runningCollapsed)); } catch { /* In-memory fallback. */ }
  }, [runningCollapsed]);
  useEffect(() => {
    try { localStorage.setItem(ARCHIVED_COLLAPSED_KEY, JSON.stringify(archivedCollapsed)); } catch { /* In-memory fallback. */ }
  }, [archivedCollapsed]);
  useEffect(() => setTemporaryCollapsed(new Set()), [searchQuery, filter]);
  useEffect(() => {
    // Reveal an explicit selection (including quick-switch and tray selections).
    const project = projects.find(project => project.id === selection?.projectId);
    if (!project) return;
    if (project.archived) setArchivedCollapsed(false);
    const group = projectGroups.find(group => group.id === project.groupId);
    if (group?.collapsed && !project.pinned && !project.archived) void onUpdateGroup(group.id, { collapsed: false }).catch(error => toast.error(String(error)));
    setTemporaryCollapsed(new Set());
    // Changes to the collapse preference itself must not reopen a manually closed group.
  }, [selection, projectsLoaded]);

  const clearDrag = useCallback(() => {
    const current = drag.current;
    drag.current = null;
    if (!current) return;
    if (current.frame !== undefined) cancelAnimationFrame(current.frame);
    current.ghost?.remove();
    if (current.button.hasPointerCapture(current.pointerId)) current.button.releasePointerCapture(current.pointerId);
    setDraggingId(null);
  }, []);
  useEffect(() => clearDrag, [clearDrag]);
  useEffect(() => {
    const current = drag.current;
    if (!current) return;
    const ids = current.kind === 'project' ? projects.map(project => project.id) : orderedGroups.map(group => group.id);
    if (!canReorder || current.ids.length !== ids.length || current.ids.some((id, index) => id !== ids[index])) clearDrag();
  }, [projects, projectGroups, canReorder, clearDrag]);

  const viewport = () => scrollArea.current?.querySelector<HTMLElement>('[data-radix-scroll-area-viewport]');
  const updateTarget = (current: ProjectDrag) => {
    const area = viewport()?.getBoundingClientRect();
    const line = indicator.current;
    if (!area || !line) return;
    current.valid = current.x >= area.left && current.x <= area.right && current.y >= area.top && current.y <= area.bottom;
    line.style.visibility = current.valid ? 'visible' : 'hidden';
    if (!current.valid) return;
    let top = area.top;
    if (current.kind === 'group') {
      const remaining = orderedGroups.filter(group => group.id !== current.id).flatMap(group => {
        const element = buckets.current.get(group.id);
        return element ? [{ id: group.id, rect: element.getBoundingClientRect() }] : [];
      });
      const before = remaining.find(item => current.y < item.rect.top + item.rect.height / 2);
      current.beforeId = before?.id ?? null;
      top = before?.rect.top ?? remaining.at(-1)?.rect.bottom ?? area.top;
    } else {
      const project = projects.find(project => project.id === current.id)!;
      const summary = runningSection.current;
      const summaryBounds = summary?.getBoundingClientRect();
      const inSummary = !!summaryBounds && current.y >= summaryBounds.top && current.y <= summaryBounds.bottom;
      if (inSummary && !runningUngroupedIds.has(current.id)) {
        current.valid = false;
        line.style.visibility = 'hidden';
        return;
      }
      const available = sections.filter(section => project.archived ? section.id === 'archived' : section.id !== 'archived' && (section.id !== 'pinned' || project.pinned)).flatMap(section => {
        const element = buckets.current.get(section.id);
        return element ? [{ section, element, rect: element.getBoundingClientRect() }] : [];
      });
      const target = available.find(item => current.y >= item.rect.top && current.y <= item.rect.bottom)
        ?? available.find(item => current.y < item.rect.top) ?? available.at(-1);
      const container = inSummary ? summary : target?.element;
      if (!container) { current.valid = false; line.style.visibility = 'hidden'; return; }
      const peers = inSummary ? runningUngrouped : target!.section.projects;
      current.targetGroupId = inSummary || target!.section.id === 'ungrouped' ? null : target!.section.group?.id ?? project.groupId;
      const remaining = peers.filter(project => project.id !== current.id).flatMap(project => {
        const element = rows.current.get(project.id);
        return element && container.contains(element) ? [{ id: project.id, rect: element.getBoundingClientRect() }] : [];
      });
      const before = remaining.find(item => current.y < item.rect.top + item.rect.height / 2);
      current.beforeId = before?.id ?? null;
      current.afterId = remaining.at(-1)?.id ?? null;
      top = before?.rect.top ?? remaining.at(-1)?.rect.bottom ?? container.querySelector('.sidebar-group-heading')?.getBoundingClientRect().bottom ?? container.getBoundingClientRect().bottom;
    }
    line.style.top = `${Math.max(area.top, Math.min(area.bottom - 2, top))}px`;
    line.style.left = `${area.left + 8}px`;
    line.style.width = `${Math.max(0, area.width - 16)}px`;
  };
  const autoScroll = (previous: number) => {
    const current = drag.current;
    if (!current?.active) return;
    current.frame = requestAnimationFrame(now => {
      if (drag.current !== current) return;
      const element = viewport();
      if (element) {
        const rect = element.getBoundingClientRect();
        const direction = current.y < rect.top + 40 ? -Math.min(1, (rect.top + 40 - current.y) / 40)
          : current.y > rect.bottom - 40 ? Math.min(1, (current.y - rect.bottom + 40) / 40) : 0;
        if (current.x >= rect.left && current.x <= rect.right && direction) {
          const before = element.scrollTop;
          element.scrollTop += direction * 600 * Math.min(50, now - previous) / 1000;
          if (element.scrollTop !== before) updateTarget(current);
        }
      }
      autoScroll(now);
    });
  };
  const startPointer = (event: ReactPointerEvent<HTMLButtonElement>, id: string, kind: ProjectDrag['kind'] = 'project') => {
    if (event.button !== 0) return;
    suppressClick.current = false;
    if (!canReorder) return;
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      id, kind, ids: kind === 'project' ? projects.map(project => project.id) : orderedGroups.map(group => group.id), pointerId: event.pointerId, button: event.currentTarget,
      startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY,
      offsetX: 0, offsetY: 0, active: false, valid: false, beforeId: null, afterId: null,
      targetGroupId: projects.find(project => project.id === id)?.groupId ?? null,
    };
  };
  const movePointer = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = drag.current;
    if (!current || event.pointerId !== current.pointerId) return;
    current.x = event.clientX;
    current.y = event.clientY;
    if (!current.active) {
      if (Math.hypot(current.x - current.startX, current.y - current.startY) < 5) return;
      const element = current.kind === 'project' ? rows.current.get(current.id) : buckets.current.get(current.id)?.querySelector<HTMLElement>('[data-group-heading]');
      if (!element) { clearDrag(); return; }
      current.active = true;
      suppressClick.current = true;
      const rect = element.getBoundingClientRect();
      current.offsetX = current.startX - rect.left;
      current.offsetY = current.startY - rect.top;
      const ghost = element.cloneNode(true) as HTMLElement;
      ghost.setAttribute('aria-hidden', 'true');
      for (const node of [ghost, ...ghost.querySelectorAll<HTMLElement>('*')]) {
        node.removeAttribute('id');
        for (const attribute of [...node.attributes]) if (attribute.name.startsWith('data-')) node.removeAttribute(attribute.name);
        if (node instanceof HTMLButtonElement) node.tabIndex = -1;
      }
      ghost.dataset.density = density;
      ghost.classList.add('sidebar-drag-preview', 'pointer-events-none', 'rounded-md', 'bg-sidebar', 'text-sidebar-foreground', 'shadow-xl', 'ring-1', 'ring-sidebar-border');
      Object.assign(ghost.style, { position: 'fixed', left: '0', top: '0', width: `${rect.width}px`, zIndex: '60' });
      document.body.appendChild(ghost);
      current.ghost = ghost;
      setDraggingId(current.id);
      autoScroll(performance.now());
    }
    event.preventDefault();
    if (current.ghost) current.ghost.style.transform = `translate3d(${current.x - current.offsetX}px, ${current.y - current.offsetY}px, 0)`;
    updateTarget(current);
  };
  const finishPointer = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = drag.current;
    if (!current || event.pointerId !== current.pointerId) return;
    const original = current.kind === 'project' ? projects.map(project => project.id) : orderedGroups.map(group => group.id);
    if (current.ids.length !== original.length || current.ids.some((id, index) => id !== original[index])) { clearDrag(); return; }
    if (current.kind === 'group') {
      const ids = original.filter(id => id !== current.id);
      ids.splice(current.beforeId ? ids.indexOf(current.beforeId) : ids.length, 0, current.id);
      const changed = current.active && current.valid && ids.some((id, index) => id !== original[index]);
      clearDrag();
      if (changed) void onReorderGroups(ids);
    } else {
      const ids = moveProjectOrder(projects, current.id, current.targetGroupId, current.beforeId, current.afterId);
      const project = projects.find(project => project.id === current.id)!;
      const moved = project.groupId !== current.targetGroupId;
      const changed = current.active && current.valid && (moved || ids.some((id, index) => id !== original[index]));
      clearDrag();
      if (changed) void onReorder(ids, moved ? { id: current.id, groupId: current.targetGroupId } : undefined);
    }
  };
  const selectOverview = (id: string) => {
    onSelect({ projectId: id, view: 'overview' });
  };
  const projectMenu = (project: ProjectConfig, x: number, y: number) => setMenu({ x, y, items: [
    { label: project.pinned ? '取消置顶' : '置顶项目', icon: project.pinned ? <PinOff /> : <Pin />, onSelect: () => { void onProjectFlags(project.id, { pinned: !project.pinned }); } },
    { label: project.archived ? '取消归档' : '归档项目', icon: project.archived ? <ArchiveRestore /> : <Archive />, onSelect: () => { void onProjectFlags(project.id, { archived: !project.archived }); } },
  ] });
  const groupMenu = (group: ProjectGroup, x: number, y: number) => setMenu({ x, y, items: [
    { label: '重命名分组', icon: <Pencil />, onSelect: () => onGroupAction('rename', group) },
    { label: '删除分组', icon: <Trash2 />, destructive: true, onSelect: () => onGroupAction('delete', group) },
  ] });

  const renderProject = (project: ProjectConfig, peers: ProjectConfig[], inRunning = false) => {
    const selectedView = selection?.projectId === project.id ? selection.view : null;
    const selected = selectedView !== null;
    const slots = getProjectTerminalSlots(project.id, terminals);
    const conflict = conflicts.some(item => item.projectIds.includes(project.id));
    const status = statuses[project.id] || 'stopped';
    const name = project.displayName || project.name;
    const shortcutIndex = visibleProjects.filter(project => !project.archived).findIndex(item => item.id === project.id);
    return <div key={project.id} ref={node => { if (node) rows.current.set(project.id, node); else rows.current.delete(project.id); }}
      data-project-id={project.id} data-project-dragging={draggingId === project.id || undefined} className={cn('flex flex-col', draggingId === project.id && 'opacity-40')}>
      <div className="sidebar-project-item sidebar-project-surface group/project relative flex items-center"
        data-selected={selected || undefined} data-highlighted={searchFocused && highlighted === project.id || undefined}
        onContextMenu={event => { event.preventDefault(); projectMenu(project, event.clientX, event.clientY); }}>
        <button className={cn('sidebar-project-row flex min-w-0 flex-1 items-center gap-2.5 rounded-[inherit] px-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring/50', canReorder && 'touch-none cursor-grab active:cursor-grabbing')}
          onPointerDown={event => startPointer(event, project.id)} onPointerMove={movePointer} onPointerUp={finishPointer} onPointerCancel={clearDrag}
          onLostPointerCapture={event => { if (drag.current?.pointerId === event.pointerId) clearDrag(); }}
          onKeyDown={event => {
            if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); projectMenu(project, rect.left, rect.bottom); return; }
            if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
            event.preventDefault();
            if (!canReorder) return;
            const index = peers.findIndex(item => item.id === project.id);
            const target = peers[index + (event.key === 'ArrowDown' ? 1 : -1)];
            if (!target) return;
            const ids = projects.map(item => item.id);
            const position = ids.indexOf(project.id), targetPosition = ids.indexOf(target.id);
            [ids[position], ids[targetPosition]] = [ids[targetPosition], ids[position]];
            void onReorder(ids);
            requestAnimationFrame(() => rows.current.get(project.id)?.querySelector<HTMLButtonElement>('[data-project-row]')?.focus({ preventScroll: true }));
          }}
          onClick={event => {
            if (suppressClick.current && event.detail !== 0) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; return; }
            selectOverview(project.id);
          }}
          title={`${name}${shortcutIndex >= 0 && shortcutIndex < 9 ? ` · ⌘${shortcutIndex + 1}` : ''}\n${filtering ? '清除搜索和筛选后可拖动排序' : '拖动排序 · ⌥↑ / ⌥↓'}`}
          data-project-row={project.id} data-running-project={inRunning ? project.id : undefined} aria-current={selected ? 'page' : undefined} data-search-highlighted={searchFocused && highlighted === project.id || undefined} data-reorder-draggable={canReorder || undefined}>
          <ProjectAvatar project={project} status={status} conflict={conflict} />
          <span className="min-w-0 flex-1 truncate">{name}</span>
          {attentionProjects.has(project.id) && <span className="size-1.5 shrink-0 rounded-full bg-attention" aria-label="终端有新动静" title="终端有新动静" data-terminal-attention={project.id} />}
          <SidebarPort project={project} hideOnHover />
        </button>
        <div className="pointer-events-none absolute right-1 flex shrink-0 items-center rounded-control opacity-0 group-hover/project:pointer-events-auto group-hover/project:opacity-100 group-focus-within/project:pointer-events-auto group-focus-within/project:opacity-100">
          <NewTerminalButton projectId={project.id} atLimit={slots.length >= maxPerProject} maxPerProject={maxPerProject} onCreate={() => onCreateTerminal(project.id)} />
          <Button size="icon-sm" variant="ghost" className="size-6 opacity-0 group-hover/project:opacity-100 group-focus-within/project:opacity-100" aria-label={`${name} 的更多操作`}
            onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); projectMenu(project, rect.left, rect.bottom); }}><MoreHorizontal className="size-3.5" /></Button>
        </div>
      </div>
      {selected && (
        <div className="sidebar-subitems flex flex-col">
          <button data-project-view="overview" data-project-target={project.id} aria-current={selectedView === 'overview' ? 'page' : undefined} data-current={selectedView === 'overview' || undefined}
            onClick={() => onSelect({ projectId: project.id, view: 'overview' })}
            className="sidebar-subrow sidebar-subitem sidebar-project-surface flex min-w-0 items-center gap-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring/50">
            <LayoutDashboard className="size-[13px] shrink-0 stroke-[1.8]" /><span className="truncate">概览</span>
          </button>
          {slots.map(slot => {
            const active = selectedView === 'terminal' && (activeSlots[project.id] ?? 1) === slot;
            const terminal = terminals[terminalKey(project.id, slot)];
            return <div key={slot} data-current={active || undefined} className="sidebar-subitem sidebar-project-surface group/terminal relative flex min-w-0 items-center">
              <button data-project-view="terminal" data-project-target={project.id} data-project-terminal-slot={slot} aria-current={active ? 'page' : undefined}
                title={terminal?.title ? `终端 ${slot} · ${terminal.title}` : undefined} onClick={() => onSelectTerminal(project.id, slot)}
                className={cn('sidebar-subrow flex min-w-0 flex-1 items-center gap-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring/50', active && (slot === 1 ? 'pr-8' : 'pr-14'))}>
                <Terminal className="size-[13px] shrink-0 stroke-[1.8]" /><TerminalRowLabel slot={slot} title={terminal?.title} />
                <span className="ml-auto flex shrink-0 group-hover/terminal:opacity-0 group-focus-within/terminal:opacity-0"><TerminalStatusDot projectId={project.id} slot={slot} status={terminal} /></span>
              </button>
              <div className={cn('absolute right-1 flex items-center rounded-control bg-[var(--sidebar-background)]', !active && 'invisible group-hover/terminal:visible group-focus-within/terminal:visible')}>
                <RestartTerminalButton projectId={project.id} slot={slot} status={terminal} active={active} onSelect={onSelectTerminal} />
                {slot > 1 && <CloseTerminalButton projectId={project.id} slot={slot} status={terminal} active={active} />}
              </div>
            </div>;
          })}
        </div>
      )}
    </div>;
  };

  const renderSection = (section: SidebarSection) => {
    const visible = section.projects.filter(project => visibleIds.has(project.id) && !runningUngroupedIds.has(project.id));
    if (filtering && !visible.length) return null;
    if (section.id === 'ungrouped' && !projectGroups.length && !visible.length && projects.length) return null;
    const open = filtering ? !temporaryCollapsed.has(section.id) : section.id === 'archived' ? !archivedCollapsed : !section.group?.collapsed;
    const members = section.group ? projects.filter(project => !project.archived && project.groupId === section.group!.id) : section.projects.filter(project => !runningUngroupedIds.has(project.id));
    const runningCount = members.filter(project => runningIds.has(project.id)).length;
    const collapsible = !!section.group || section.id === 'archived';
    return <div key={section.id} ref={node => { if (node) buckets.current.set(section.id, node); else buckets.current.delete(section.id); }}
      data-sidebar-bucket={section.id} className={cn('min-w-0', draggingId === section.id && 'opacity-40')}>
      <div data-group-heading className="sidebar-group-heading flex min-w-0 items-center"
        onContextMenu={event => { if (section.group) { event.preventDefault(); groupMenu(section.group, event.clientX, event.clientY); } }}>
        <button className="flex min-w-0 flex-1 items-center gap-[7px] rounded text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50" disabled={!collapsible}
          data-group-toggle={section.id} aria-expanded={collapsible ? open : undefined} aria-controls={collapsible ? `sidebar-section-${section.id}` : undefined}
          onClick={() => {
            if (filtering) setTemporaryCollapsed(previous => { const next = new Set(previous); if (open) next.add(section.id); else next.delete(section.id); return next; });
            else if (section.id === 'archived') setArchivedCollapsed(previous => !previous);
            else if (section.group) void onUpdateGroup(section.id, { collapsed: open }).catch(error => toast.error(`保存折叠状态失败：${error}`));
          }}>
          {section.id === 'pinned' ? <Pin aria-hidden="true" className="size-3 shrink-0" />
            : (collapsible || projectGroups.length > 0) && <GroupHeadingMarker color={groupColor(section.group?.name)} expanded={collapsible ? open : undefined} />}
          <span className="truncate" title={section.name}>{section.name}</span>
          {runningCount > 0 && <span className="ml-1.5 shrink-0 rounded-full bg-[rgba(16,185,129,.12)] px-[7px] py-px text-[10px] leading-[14px] text-[#047857]" data-group-running={section.id}>{runningCount} 运行</span>}
        </button>
        <div className="flex h-0 shrink-0 items-center">
          {section.group && <>
            <Button size="icon-sm" variant="ghost" className={cn('sidebar-heading-action size-[18px]', canReorder && 'touch-none cursor-grab')} aria-label={`拖动分组 ${section.name}`} disabled={!canReorder} data-group-drag={section.id}
              onPointerDown={event => startPointer(event, section.id, 'group')} onPointerMove={movePointer} onPointerUp={finishPointer} onPointerCancel={clearDrag}
              onLostPointerCapture={event => { if (drag.current?.pointerId === event.pointerId) clearDrag(); }}
              onKeyDown={event => {
                if (!canReorder || !event.altKey || event.ctrlKey || event.metaKey || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
                event.preventDefault();
                const ids = orderedGroups.map(group => group.id), index = ids.indexOf(section.id), target = index + (event.key === 'ArrowDown' ? 1 : -1);
                if (target >= 0 && target < ids.length) { [ids[index], ids[target]] = [ids[target], ids[index]]; void onReorderGroups(ids); }
              }}><GripVertical className="size-3" /></Button>
            <Button size="icon-sm" variant="ghost" className="sidebar-heading-action size-[18px]" aria-label={`${section.name} 分组的更多操作`}
              onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); groupMenu(section.group!, rect.left, rect.bottom); }}><MoreHorizontal className="size-3.5" /></Button>
          </>}
          {(section.group || section.id === 'ungrouped') && <ActionTooltip label="新建分组"><Button variant="ghost" size="icon-sm" className="sidebar-heading-action size-[18px]" aria-label="新建分组" data-create-group onClick={() => onGroupAction('create')}><FolderPlus className="size-3.5" /></Button></ActionTooltip>}
        </div>
        <span className="sidebar-number font-medium" data-group-count={section.id}>{filtering ? `${visible.length} / ${members.length}` : members.length}</span>
      </div>
      {open && <div id={`sidebar-section-${section.id}`} className="sidebar-project-list">
        {visible.map(project => renderProject(project, visible))}
        {!filtering && !visible.length && section.group && <p className="px-7 py-2 text-[10px] text-muted-foreground/60">拖入项目到此分组</p>}
      </div>}
    </div>;
  };
  const renderRunning = () => !filtering && runningProjects.length > 0 && <section ref={runningSection} aria-labelledby="sidebar-running-heading" data-running-section>
    <button className="sidebar-group-heading flex w-full items-center gap-[7px] text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      aria-label={runningCollapsed ? '展开运行中项目' : '收起运行中项目'} aria-expanded={!runningCollapsed} aria-controls="sidebar-running-projects" data-running-toggle onClick={() => setRunningCollapsed(previous => !previous)}>
      <GroupHeadingMarker color="#10b981" expanded={!runningCollapsed} /><span id="sidebar-running-heading">运行中</span>
      <span className="sidebar-number ml-auto font-medium" data-running-count>{running}</span>
    </button>
    {!runningCollapsed && <ScrollArea className="[&>[data-radix-scroll-area-viewport]]:max-h-[28vh]">
      <nav id="sidebar-running-projects" aria-label="运行中的项目" className="sidebar-project-list">
        {runningProjects.map(project => {
          if (runningUngroupedIds.has(project.id)) return renderProject(project, runningUngrouped, true);
          const selected = selection?.projectId === project.id;
          return <button key={project.id} data-running-project={project.id} data-selected={selected || undefined} aria-current={selected && selection.view === 'overview' ? 'page' : undefined}
            className="sidebar-project-item sidebar-project-row sidebar-project-surface flex w-full shrink-0 items-center gap-2.5 px-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            onClick={() => selectOverview(project.id)} onContextMenu={event => { event.preventDefault(); projectMenu(project, event.clientX, event.clientY); }}>
            <ProjectAvatar project={project} status={statuses[project.id]} />
            <span className="min-w-0 flex-1 truncate">{project.displayName || project.name}</span>
            {attentionProjects.has(project.id) && <span className="size-1.5 shrink-0 rounded-full bg-attention" aria-label="终端有新动静" />}
            <SidebarPort project={project} />
          </button>;
        })}
      </nav>
    </ScrollArea>}
  </section>;

  return <aside className="app-sidebar relative flex shrink-0 flex-col text-sidebar-foreground" aria-label="项目侧边栏" data-density={density} data-native-vibrancy={nativeVibrancy || undefined} data-folder-drag={folderDropName ? 'true' : undefined}
    onKeyDownCapture={event => { if (event.key === 'Escape' && drag.current?.active) { event.preventDefault(); event.stopPropagation(); clearDrag(); } }}>
    <div className="sidebar-glass" aria-hidden="true" />
    <div data-tauri-drag-region className="h-[46px] shrink-0" />
    <header data-tauri-drag-region className="flex shrink-0 items-center gap-2.5 px-4 pb-3.5">
      <img src={logo} alt="" className="pointer-events-none size-8 shrink-0" />
      <div className="pointer-events-none min-w-0 flex-1"><h1 className="text-brand font-heading leading-[21px] tracking-title text-foreground">Bento</h1>
        <p className="mt-px truncate text-meta leading-4 text-muted-foreground">{projects.length} 个项目 · <span className="font-semibold text-success" data-running-header>{running} 运行中</span></p></div>
      <div className="no-drag flex shrink-0 items-center gap-2.5">
        <ActionTooltip label="刷新项目状态" side="bottom"><Button variant="ghost" size="icon-sm" aria-label="刷新项目状态" onClick={onRefresh} disabled={loading}><RefreshCw className={cn(loading && 'animate-spin')} /></Button></ActionTooltip>
        <ActionTooltip label="添加项目 ⌘N" side="bottom" align="end"><Button size="sm" className="h-[31px] gap-1 rounded-[8px] px-3 font-semibold shadow-none hover:ring-[3px] hover:ring-primary/15" aria-label="添加项目" aria-keyshortcuts="Meta+n" onClick={onCreate} data-create-project><Plus data-icon="inline-start" />添加</Button></ActionTooltip>
      </div>
    </header>
    <div className="relative mx-3 shrink-0">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3 -translate-y-1/2 text-[#a1a1aa]" />
      <Input ref={searchInputRef} type="search" placeholder="搜索或跳转项目…" aria-label="搜索项目" aria-controls="project-list" data-project-search
        className="sidebar-search h-[35px] border-0 pl-[30px] pr-14 placeholder:text-[#a1a1aa] [&::-webkit-search-cancel-button]:hidden" value={searchQuery}
        onFocus={() => setSearchFocused(true)} onBlur={() => setSearchFocused(false)} onChange={event => { onSearchChange(event.target.value); setHighlightedId(null); }}
        onKeyDown={event => {
          if (event.nativeEvent.isComposing || event.keyCode === 229) return;
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            const index = Math.max(0, visibleProjects.findIndex(project => project.id === highlighted));
            const next = visibleProjects[Math.max(0, Math.min(visibleProjects.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
            if (next) { setHighlightedId(next.id); rows.current.get(next.id)?.scrollIntoView({ block: 'nearest' }); }
          } else if (event.key === 'Enter') { event.preventDefault(); if (highlighted) selectOverview(highlighted); }
          else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); if (searchQuery) { onSearchChange(''); setHighlightedId(null); } else event.currentTarget.blur(); }
        }} />
      {searchQuery ? <Button variant="ghost" size="icon-sm" className="absolute right-1 top-1/2 -translate-y-1/2" aria-label="清除搜索" onClick={() => { onSearchChange(''); setHighlightedId(null); searchInputRef.current?.focus(); }}><X className="size-3.5" /></Button>
        : <button aria-label="快速切换项目 (⌘P)" aria-keyshortcuts="Meta+p" title="快速切换项目 (⌘P)" className="absolute right-2.5 top-1/2 -translate-y-1/2 rounded border border-black/[.08] px-[5px] py-px text-keycap leading-[15px] text-muted-foreground hover:bg-[var(--sidebar-hover)]" onClick={onQuickSwitch}>⌘P</button>}
    </div>
    {projects.length > 0 && <SegmentedControl label="项目状态筛选" className="mx-3 mb-1 mt-2.5 shrink-0" value={filter} onChange={onFilterChange}
      options={([{ id: 'all', label: '全部', count: null }, { id: 'running', label: '运行中', count: running },
        { id: 'attention', label: '提醒', count: projects.filter(project => attentionProjects.has(project.id)).length },
        { id: 'changes', label: '改动', count: projects.filter(project => changedProjects.has(project.id)).length }] as const).map(item => ({
          value: item.id, attributes: { 'data-sidebar-filter': item.id },
          label: <>{item.label}{item.count != null && <span className="sidebar-filter-count tabular-nums">{item.count}</span>}</>,
        }))} />}
    <ScrollArea ref={scrollArea} className="min-h-0 flex-1 px-3 pb-2">
      <nav id="project-list" aria-label="项目列表" className="flex min-h-full flex-col">
        {projectsLoaded && !loading && projects.length === 0 ? <div className="mx-2 mt-6 rounded-lg border border-dashed border-muted-foreground/35 px-3 py-5 text-center" data-sidebar-empty>
          <p className="text-sm font-medium text-foreground">还没有项目</p>
          <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">把项目文件夹拖到这里</p>
          <p className="mt-0.5 text-[11px] text-muted-foreground">或者</p>
          <Button size="sm" className="mt-3 gap-1 px-2.5 text-[11px]" onClick={onCreate} data-add-first-project><Plus className="!size-3.5" />添加第一个项目</Button>
        </div> : <>
        {filtering && <p className="px-2 pb-1 pt-2 text-[10px] leading-relaxed text-muted-foreground" data-reorder-search-hint>清除搜索和筛选后可拖动排序</p>}
        {filtering && !visibleProjects.length && <p role="status" className="px-2 py-6 text-xs leading-relaxed text-muted-foreground break-all" data-project-search-empty>{searching ? `没有匹配「${searchQuery.trim()}」的项目` : '没有符合筛选条件的项目'}</p>}
        {sections.filter(section => section.id === 'pinned').map(renderSection)}
        {renderRunning()}
        {sections.filter(section => section.id !== 'pinned').map(renderSection)}
        </>}
      </nav>
    </ScrollArea>
    <footer className="sidebar-footer mx-3 flex shrink-0 items-center gap-2 px-1.5 pb-3 pt-2.5 text-meta text-muted-foreground" data-update-footer data-managed-footer={managedCount > 0 || undefined}>
      <div className="flex min-w-0 items-center gap-1">
        {running > 0 && <>
          <span aria-hidden="true" className="mr-0.5 size-1.5 shrink-0 rounded-full bg-[#10b981]" />
          <span className="whitespace-nowrap" data-running-footer>{running} 运行中</span>
          <span aria-hidden="true">·</span>
        </>}
        <ActionTooltip label={updateLabel} side="right">
          <Button variant="ghost" size="sm" className="h-6 min-w-0 gap-1 px-0.5 text-meta font-normal hover:bg-[var(--sidebar-hover)]" aria-label={currentVersion ? `${versionLabel}，${updateLabel}` : updateLabel} onClick={onCheckUpdates} data-check-updates>
            <span className="truncate">{versionLabel}</span>
            {availableVersion && <ArrowDownToLine aria-hidden="true" className="text-foreground" />}
          </Button>
        </ActionTooltip>
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        {managedCount > 0 && <Button variant="outline" size="sm" className="h-6 border-0 bg-background/70 px-[9px] text-meta font-normal text-foreground shadow-[0_0_0_.5px_rgba(0,0,0,.12)]" onClick={onStopAll} data-stop-all>全部停止</Button>}
        <ActionTooltip label="设置" side="right">
          <Button variant="ghost" size="icon-sm" className="size-6 hover:bg-[var(--sidebar-hover)]" aria-label="设置" onClick={onOpenSettings} data-open-settings><Settings /></Button>
        </ActionTooltip>
      </div>
    </footer>
    {folderDropName && <div className="pointer-events-none absolute inset-2 z-40 flex flex-col items-center justify-center gap-3 rounded-xl border-2 border-dashed border-success bg-success-muted/95 px-4 text-center text-success-foreground backdrop-blur-[1px]" role="status" aria-live="polite" data-folder-drop-overlay>
      <FolderOpen className="mb-1 size-12 text-success/75" strokeWidth={1.5} />
      <p className="max-w-full break-words text-sm font-semibold">松开以添加 {folderDropName}</p>
      <p className="text-[11px] leading-relaxed text-success">会自动识别启动命令和端口</p>
    </div>}
    <div ref={indicator} hidden={!draggingId} aria-hidden="true" data-reorder-indicator className="pointer-events-none fixed z-[70] h-0.5 rounded-full bg-attention" />
    {menu && <SidebarMenu {...menu} onClose={closeMenu} />}
  </aside>;
});
