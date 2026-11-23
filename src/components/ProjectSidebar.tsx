import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent, RefObject } from 'react';
import { ArrowDownToLine, Check, ChevronRight, FolderOpen, FolderPlus, GripVertical, ListFilter, MoreHorizontal, Pencil, Pin, Plus, RefreshCw, Search, Settings, Square, Trash2, X } from 'lucide-react';
import { toast } from 'sonner';
import type { PortConflict, ProjectConfig, ProjectGroup, ProjectSelection, ProjectStatus, Snippet, TerminalCompletion } from '../types';
import { moveProjectOrder, sidebarSections, type SidebarFilter, type SidebarSection } from '../lib/sidebarModel';
import { getProjectTerminalSlots, getTerminalSnapshot, subscribeTerminals } from '../lib/terminalRegistry';
import { agentMinutesSince, agentNames, agentStateLabels, terminalDisplayNumber, type ProjectAgent } from '../lib/terminalUsability';
import { cn } from '../lib/utils';
import { recordRender } from '../lib/renderMetrics';
import { useSidebarMotion } from '../hooks/useSidebarMotion';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';
import { SidebarScrollArea } from './SidebarScrollArea';
import { Input } from './ui/input';
import { SegmentedControl } from './ui/segmented-control';
import { ProjectAvatar } from './ProjectAvatar';
import { AgentElapsedTime } from './AgentElapsedTime';
import { StatusDot } from './StatusDot';
import { SidebarMenu, type SidebarMenuItem } from './SidebarMenu';
import { ProjectActionsMenu } from './ProjectActionsMenu';
import { MotionPresence } from './ui/motion-presence';

interface Props {
  projects: ProjectConfig[];
  inactive: boolean;
  folderDropName: string | null;
  groups: ProjectGroup[];
  filter: SidebarFilter;
  onFilterChange: (filter: SidebarFilter) => void;
  attentionProjects: ReadonlySet<string>;
  projectAgents: ReadonlyMap<string, ProjectAgent>;
  completions: ReadonlyMap<string, TerminalCompletion>;
  snippets: Record<string, Snippet[]>;
  onRunSnippet: (id: string, command: string) => void;
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
  maxPerProject: number;
  loading: boolean;
  projectsLoaded: boolean;
  onSelectProject: (projectId: string) => void;
  onCreateTerminal: (projectId: string) => Promise<void>;
  onOpenProjectLogs: (project: ProjectConfig) => void;
  onDeleteProject: (project: ProjectConfig) => void;
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

const ARCHIVED_COLLAPSED_KEY = 'bento.sidebar.archivedCollapsed';

function GroupHeadingMarker({ expanded }: { expanded: boolean }) {
  return <ChevronRight aria-hidden="true"
    className={cn('sidebar-heading-disclosure ml-auto size-2.5 shrink-0 motion-safe:transition-transform', expanded && 'rotate-90')} />;
}

function ProjectRowStatus({ projectId, status, conflict, attention, completion }: {
  projectId: string; status: ProjectStatus; conflict?: boolean; attention?: boolean; completion?: TerminalCompletion;
}) {
  if (completion) return <span className="sidebar-completion" data-command-completion={completion.exitCode === 0 ? 'done' : 'failed'}
    title={completion.command + ' · 退出码 ' + completion.exitCode}>{completion.exitCode === 0 ? '完成' : '× 失败'}</span>;
  if (status === 'stopped' && !conflict && !attention) return null;
  const runtimeLabel = conflict ? '端口冲突' : { running: '运行中', starting: '启动中', error: '运行异常', stopped: '' }[status];
  const label = [runtimeLabel, attention && '终端有新动静'].filter(Boolean).join(' · ');
  return <StatusDot title={label} aria-label={label} data-terminal-attention={attention ? projectId : undefined}
    tone={conflict ? 'error' : status} />;
}

export const ProjectSidebar = memo(function ProjectSidebar({ projects, inactive, folderDropName, groups: projectGroups, filter, onFilterChange, attentionProjects, projectAgents, completions, snippets, onRunSnippet, onQuickSwitch, onUpdateGroup, onReorderGroups, onGroupAction, onProjectFlags, visibleProjects, searchQuery, searchInputRef, onSearchChange, onReorder, reordering, statuses, conflicts, selection, maxPerProject, loading, projectsLoaded, onSelectProject, onCreateTerminal, onOpenProjectLogs, onDeleteProject, onRefresh, onCreate, managedCount, onStopAll, currentVersion, availableVersion, onCheckUpdates, onOpenSettings }: Props) {
  recordRender('projectSidebar');
  const [archivedCollapsed, setArchivedCollapsed] = useState(() => {
    try { return JSON.parse(localStorage.getItem(ARCHIVED_COLLAPSED_KEY) ?? 'true') !== false; } catch { return true; }
  });
  const [temporaryCollapsed, setTemporaryCollapsed] = useState(new Set<string>());
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [searchFocused, setSearchFocused] = useState(false);
  const [, setCompletionClock] = useState(0);
  const hasAgentCompletion = [...projectAgents.values()].some(({ agent }) => agent.state === 'done');
  useEffect(() => {
    if (!hasAgentCompletion || inactive) return;
    const timer = setInterval(() => setCompletionClock(Date.now()), 15_000);
    return () => clearInterval(timer);
  }, [hasAgentCompletion, inactive]);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: SidebarMenuItem[] } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const [rowMenu, setRowMenu] = useState<{ x: number; y: number; projectId: string; keyboard: boolean }>();
  const closeRowMenu = useCallback(() => setRowMenu(undefined), []);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const buckets = useRef(new Map<string, HTMLDivElement>());
  const scrollArea = useRef<HTMLDivElement>(null);
  const projectList = useRef<HTMLElement>(null);
  const selectionMarker = useRef<HTMLSpanElement>(null);
  useSidebarMotion(projectList, selectionMarker, projects, projectsLoaded, inactive);
  const indicator = useRef<HTMLDivElement>(null);
  const drag = useRef<ProjectDrag | null>(null);
  const suppressClick = useRef(false);
  const searching = searchQuery.trim().length > 0;
  const filtering = searching || filter !== 'all';
  const canReorder = !filtering && !loading && !reordering;
  const highlighted = visibleProjects.find(project => project.id === highlightedId)?.id || visibleProjects[0]?.id;
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const menuProject = rowMenu && projects.find(project => project.id === rowMenu.projectId);
  const menuTerminalCount = menuProject
    ? Object.values(terminals).filter(terminal => terminal.projectId === menuProject.id && terminal.sessionId).length : 0;
  const visibleIds = useMemo(() => new Set(visibleProjects.map(project => project.id)), [visibleProjects]);
  const sections = useMemo(() => sidebarSections(projects, projectGroups), [projects, projectGroups]);
  const orderedGroups = [...projectGroups].sort((a, b) => a.sortOrder - b.sortOrder || a.id.localeCompare(b.id));
  const runningCount = projects.filter(project => statuses[project.id] === 'running').length;
  const versionLabel = currentVersion ? `v${currentVersion}` : '检查更新';
  const updateLabel = availableVersion ? `更新到 v${availableVersion}` : '检查更新';
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
    if (inactive) { closeMenu(); closeRowMenu(); clearDrag(); }
  }, [inactive, closeMenu, closeRowMenu, clearDrag]);
  useEffect(() => {
    const current = drag.current;
    if (!current) return;
    const ids = current.kind === 'project' ? projects.map(project => project.id) : orderedGroups.map(group => group.id);
    if (!canReorder || current.ids.length !== ids.length || current.ids.some((id, index) => id !== ids[index])) clearDrag();
  }, [projects, projectGroups, canReorder, clearDrag]);

  const viewport = () => scrollArea.current?.querySelector<HTMLElement>('[data-sidebar-scroll-viewport]');
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
      const available = sections.filter(section => project.archived ? section.id === 'archived' : section.id !== 'archived' && (section.id !== 'pinned' || project.pinned)).flatMap(section => {
        const element = buckets.current.get(section.id);
        return element ? [{ section, element, rect: element.getBoundingClientRect() }] : [];
      });
      const target = available.find(item => current.y >= item.rect.top && current.y <= item.rect.bottom)
        ?? available.find(item => current.y < item.rect.top) ?? available.at(-1);
      if (!target) { current.valid = false; line.style.visibility = 'hidden'; return; }
      const { element: container, section } = target;
      current.targetGroupId = section.id === 'ungrouped' ? null : section.group?.id ?? project.groupId;
      const remaining = section.projects.filter(project => project.id !== current.id).flatMap(project => {
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
    if (event.button !== 0 || event.ctrlKey) return;
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
      ghost.classList.add('sidebar-drag-preview', 'h1-popup', 'pointer-events-none');
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
  const projectMenu = (projectId: string, x: number, y: number, keyboard = false) => {
    closeMenu();
    setRowMenu({ x, y, projectId, keyboard });
  };
  const projectMenuFromKeyboard = (event: ReactKeyboardEvent<HTMLButtonElement>, projectId: string) => {
    if (event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10')) return false;
    event.preventDefault();
    const { left, bottom } = event.currentTarget.getBoundingClientRect();
    projectMenu(projectId, left, bottom, true);
    return true;
  };
  const groupMenu = (group: ProjectGroup, x: number, y: number) => {
    closeRowMenu();
    setMenu({ x, y, items: [
      { label: '重命名分组', icon: <Pencil />, onSelect: () => onGroupAction('rename', group) },
      { label: '删除分组', icon: <Trash2 />, destructive: true, onSelect: () => onGroupAction('delete', group) },
    ] });
  };

  const renderProject = (project: ProjectConfig, peers: ProjectConfig[]) => {
    const selected = selection?.projectId === project.id;
    const conflict = conflicts.some(item => item.projectIds.includes(project.id));
    const status = statuses[project.id] || 'stopped';
    const name = project.displayName || project.name;
    const shortcutIndex = visibleProjects.filter(project => !project.archived).findIndex(item => item.id === project.id);
    const choice = projectAgents.get(project.id);
    const agent = choice?.agent;
    const number = choice && terminalDisplayNumber(getProjectTerminalSlots(project.id), choice.slot);
    const minutesSinceCompletion = agent?.state === 'done' ? agentMinutesSince(agent.stateStartedAt, Date.now()) : 0;
    const agentDetail = choice && [
      agentNames[choice.agent.kind],
      number && (choice.multiple || choice.agent.state === 'waiting') && `终端 ${number}`,
      choice.agent.state === 'done' && (minutesSinceCompletion ? `${minutesSinceCompletion} 分钟前` : '刚刚'),
      choice.agent.state === 'thinking' && !choice.multiple && agentStateLabels.thinking,
    ].filter(Boolean).join(' · ');
    return <div key={project.id} ref={node => { if (node) rows.current.set(project.id, node); else rows.current.delete(project.id); }}
      data-project-id={project.id} data-project-dragging={draggingId === project.id || undefined} className={cn('flex flex-col', draggingId === project.id && 'opacity-40')}>
      <div className="sidebar-project-item sidebar-project-surface relative flex items-center"
        data-selected={selected || undefined} data-highlighted={searchFocused && highlighted === project.id || undefined}
        onContextMenu={event => { event.preventDefault(); projectMenu(project.id, event.clientX, event.clientY); }}>
        <button className={cn('sidebar-project-row flex min-w-0 flex-1 items-center rounded-[inherit] text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/25', canReorder && 'touch-none cursor-grab active:cursor-grabbing')}
          onPointerDown={event => startPointer(event, project.id)} onPointerMove={movePointer} onPointerUp={finishPointer} onPointerCancel={clearDrag}
          onLostPointerCapture={event => { if (drag.current?.pointerId === event.pointerId) clearDrag(); }}
          onKeyDown={event => {
            if (projectMenuFromKeyboard(event, project.id)) return;
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
            if (event.ctrlKey) return;
            if (suppressClick.current && event.detail !== 0) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; return; }
            onSelectProject(project.id);
          }}
          title={`${name}${shortcutIndex >= 0 && shortcutIndex < 9 ? ` · ⌘${shortcutIndex + 1}` : ''}\n${filtering ? '清除搜索和筛选后可拖动排序' : '拖动排序 · ⌥↑ / ⌥↓'}`}
          data-project-row={project.id} data-project-agent={agent?.state} aria-current={selected ? 'page' : undefined}
          aria-haspopup="menu" data-search-highlighted={searchFocused && highlighted === project.id || undefined} data-reorder-draggable={canReorder || undefined}>
          <ProjectAvatar project={project} status={status} conflict={conflict} />
          <span className="min-w-0 flex-1">
            <span className="block truncate">{name}</span>
            {agentDetail && <span className="sidebar-agent-detail block truncate" title={agentDetail}>{agentDetail}</span>}
          </span>
          {agent ? <span className="agent-status-chip" data-agent-state={agent.state}>
            {agent.state === 'thinking' ? <>
              <span className="agent-thinking-ring" aria-hidden="true" />
              {choice?.multiple && <span className="sr-only">思考中 · </span>}
              <AgentElapsedTime startAt={agent.roundStartedAt ?? agent.stateStartedAt} compact paused={inactive} />
            </> : agentStateLabels[agent.state]}
          </span> : <ProjectRowStatus projectId={project.id} status={status} conflict={conflict} attention={attentionProjects.has(project.id)} completion={completions.get(project.id)} />}
        </button>
      </div>
    </div>;
  };

  const renderSection = (section: SidebarSection) => {
    const visible = section.projects.filter(project => visibleIds.has(project.id));
    if (filtering && !visible.length) return null;
    if (section.id === 'ungrouped' && !visible.length && projects.length && !draggingId) return null;
    const open = filtering ? !temporaryCollapsed.has(section.id) : section.id === 'archived' ? !archivedCollapsed : !section.group?.collapsed;
    const collapsible = !!section.group || section.id === 'archived';
    return <div key={section.id} ref={node => { if (node) buckets.current.set(section.id, node); else buckets.current.delete(section.id); }}
      data-sidebar-bucket={section.id} className={cn('sidebar-group min-w-0', draggingId === section.id && 'opacity-40')}>
      <div data-group-heading className="sidebar-group-heading flex min-w-0 items-center"
        onContextMenu={event => { if (section.group) { event.preventDefault(); groupMenu(section.group, event.clientX, event.clientY); } }}>
        <button className="sidebar-group-toggle flex min-w-0 flex-1 items-center gap-1.5 rounded text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/25" disabled={!collapsible}
          data-group-toggle={section.id} aria-expanded={collapsible ? open : undefined} aria-controls={collapsible ? `sidebar-section-${section.id}` : undefined}
          onClick={() => {
            if (filtering) setTemporaryCollapsed(previous => { const next = new Set(previous); if (open) next.add(section.id); else next.delete(section.id); return next; });
            else if (section.id === 'archived') setArchivedCollapsed(previous => !previous);
            else if (section.group) void onUpdateGroup(section.id, { collapsed: open }).catch(error => toast.error(`保存折叠状态失败：${error}`));
          }}>
          {section.id === 'pinned' && <Pin aria-hidden="true" className="size-3 shrink-0" />}
          <span className="truncate" title={section.name}>{section.name}</span>
          <span className="sidebar-group-count">{visible.length}</span>
          {collapsible && <GroupHeadingMarker expanded={open} />}
        </button>
        <div className="sidebar-group-actions flex items-center" data-disclosure={collapsible || undefined}>
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
      </div>
      {open && <div id={`sidebar-section-${section.id}`} className="sidebar-project-list">
        {visible.map(project => renderProject(project, visible))}
        {!filtering && !visible.length && section.group && <p className="px-7 py-2 text-[10px] text-muted-foreground/60">拖入项目到此分组</p>}
      </div>}
    </div>;
  };
  const sidebarActions = (button: HTMLButtonElement) => {
    closeRowMenu();
    const rect = button.getBoundingClientRect();
    const filters = [
      { value: 'attention', label: '终端提醒' },
      { value: 'changes', label: '有改动' },
    ] as const;
    setMenu({ x: rect.right - 160, y: rect.bottom + 6, items: [
      ...filters.map(item => ({ label: item.label,
        icon: filter === item.value ? <Check /> : <span className="size-3.5" />,
        current: filter === item.value, onSelect: () => onFilterChange(item.value) })),
      { label: '刷新项目状态', icon: <RefreshCw />, disabled: loading, onSelect: onRefresh },
      { label: updateLabel + (currentVersion && !availableVersion ? ' · ' + versionLabel : ''), icon: <ArrowDownToLine />, onSelect: onCheckUpdates },
      ...(managedCount > 0 ? [{ label: '全部停止', icon: <Square />, onSelect: onStopAll }] : []),
    ] });
  };

  return <aside className="app-sidebar flex flex-col" aria-label="项目侧边栏" data-folder-drag={folderDropName ? 'true' : undefined}
    {...{ inert: inactive ? '' : undefined }}
    onKeyDownCapture={event => { if (event.key === 'Escape' && drag.current?.active) { event.preventDefault(); event.stopPropagation(); clearDrag(); } }}>
    <header data-tauri-drag-region="deep" className="sidebar-titlebar">
      <div className="sidebar-chrome">
        <ActionTooltip label="项目筛选与操作" side="bottom">
          <Button size="icon-sm" variant="ghost" className="size-[26px] rounded-full" aria-label="项目筛选与操作" aria-haspopup="menu" data-sidebar-tools
            data-filter-active={filter !== 'all' || undefined} onClick={event => sidebarActions(event.currentTarget)}>
            {filter !== 'all' ? <ListFilter /> : <MoreHorizontal />}
          </Button>
        </ActionTooltip>
      </div>
    </header>
    <div className="sidebar-search-wrap relative shrink-0">
      <Search className="pointer-events-none absolute left-3 top-1/2 size-3 -translate-y-1/2 text-[var(--h1-label)]" />
      <Input ref={searchInputRef} type="search" placeholder="搜索项目" aria-label="搜索项目" aria-controls="project-list" data-project-search
        className="sidebar-search [&::-webkit-search-cancel-button]:hidden" value={searchQuery}
        onFocus={() => setSearchFocused(true)} onBlur={() => setSearchFocused(false)} onChange={event => { onSearchChange(event.target.value); setHighlightedId(null); }}
        onKeyDown={event => {
          if (event.nativeEvent.isComposing || event.keyCode === 229) return;
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            const index = Math.max(0, visibleProjects.findIndex(project => project.id === highlighted));
            const next = visibleProjects[Math.max(0, Math.min(visibleProjects.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
            if (next) { setHighlightedId(next.id); rows.current.get(next.id)?.scrollIntoView({ block: 'nearest' }); }
          } else if (event.key === 'Enter') { event.preventDefault(); if (highlighted) onSelectProject(highlighted); }
          else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); if (searchQuery) { onSearchChange(''); setHighlightedId(null); } else event.currentTarget.blur(); }
        }} />
      {searchQuery ? <Button variant="ghost" size="icon-sm" className="absolute right-1 top-1/2 -translate-y-1/2" aria-label="清除搜索" onClick={() => { onSearchChange(''); setHighlightedId(null); searchInputRef.current?.focus(); }}><X className="size-3.5" /></Button>
        : <button aria-label="快速切换项目 (⌘P)" aria-keyshortcuts="Meta+p" title="快速切换项目 (⌘P)" className="absolute right-3 top-1/2 -translate-y-1/2 rounded py-1 text-[11px] leading-[15px] text-[var(--h1-label)] hover:bg-[var(--h1-hover)]" onClick={onQuickSwitch}>⌘P</button>}
    </div>
    <SegmentedControl<SidebarFilter> className="sidebar-filters" label="筛选项目" value={filter} onChange={onFilterChange}
      options={[
        { value: 'all', label: '全部', attributes: { 'data-sidebar-filter': 'all' } },
        { value: 'running', label: '运行中 ' + runningCount, attributes: { 'data-sidebar-filter': 'running' } },
        { value: 'pinned', label: '置顶', attributes: { 'data-sidebar-filter': 'pinned' } },
      ]} />
    <SidebarScrollArea ref={scrollArea} className="sidebar-scroll min-h-0 flex-1">
      <nav ref={projectList} id="project-list" aria-label="项目列表" className="relative flex min-h-full flex-col">
        <span ref={selectionMarker} className="sidebar-selection-marker" aria-hidden="true" hidden />
        {projectsLoaded && !loading && projects.length === 0 && filter === 'all' ? <div className="h1-group mt-4 px-3 py-5 text-center" data-sidebar-empty>
          <p className="text-sm font-medium text-foreground">还没有项目</p>
          <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">把项目文件夹拖到这里</p>
          <p className="mt-0.5 text-[11px] text-muted-foreground">或者</p>
          <Button size="sm" className="mt-3 gap-1 px-2.5 text-[11px]" onClick={onCreate} data-add-first-project><Plus className="!size-3.5" />添加第一个项目</Button>
        </div> : <>
        {filtering && <p className="px-2 pb-1 pt-2 text-[10px] leading-relaxed text-muted-foreground" data-reorder-search-hint>清除搜索和筛选后可拖动排序</p>}
        {filtering && !visibleProjects.length && <p role="status" className="h1-group mt-2 break-all px-3 py-5 text-xs leading-relaxed text-muted-foreground" data-project-search-empty>{searching ? `没有匹配「${searchQuery.trim()}」的项目` : filter === 'running' ? '没有运行中的项目' : filter === 'pinned' ? '没有置顶项目' : '没有符合筛选条件的项目'}</p>}
        {sections.map(renderSection)}
        </>}
      </nav>
    </SidebarScrollArea>
    <footer className="sidebar-footer" data-update-footer data-managed-footer={managedCount > 0 || undefined}>
      <Button variant="outline" className="min-w-0 flex-1 rounded-full" aria-label="添加项目" aria-keyshortcuts="Meta+n" onClick={onCreate} data-create-project>
        <Plus data-icon="inline-start" />添加项目
      </Button>
      <ActionTooltip label="设置" side="right">
        <Button variant="outline" className="sidebar-settings rounded-full" aria-label="设置" onClick={onOpenSettings} data-open-settings><Settings /></Button>
      </ActionTooltip>
    </footer>
    {folderDropName && <div className="pointer-events-none absolute inset-2 z-40 flex flex-col items-center justify-center gap-3 rounded-xl border-2 border-dashed border-border bg-popover px-4 text-center text-foreground" role="status" aria-live="polite" data-folder-drop-overlay>
      <FolderOpen className="mb-1 size-12 text-muted-foreground" strokeWidth={1.5} />
      <p className="max-w-full break-words text-sm font-semibold">松开以添加 {folderDropName}</p>
      <p className="text-[11px] leading-relaxed text-muted-foreground">会自动识别启动命令和端口</p>
    </div>}
    <div ref={indicator} hidden={!draggingId} aria-hidden="true" data-reorder-indicator className="pointer-events-none fixed z-[70] h-0.5 rounded-full bg-foreground/60" />
    <MotionPresence>{!inactive && menu && <SidebarMenu {...menu} onClose={closeMenu} />}</MotionPresence>
    <MotionPresence>{!inactive && rowMenu && menuProject && <ProjectActionsMenu x={rowMenu.x} y={rowMenu.y} keyboard={rowMenu.keyboard} project={menuProject} onClose={closeRowMenu}
      snippets={snippets[menuProject.id] ?? []} onRunSnippet={command => onRunSnippet(menuProject.id, command)}
      onOpenLogs={() => onOpenProjectLogs(menuProject)} onDelete={() => onDeleteProject(menuProject)} sidebar={{
        onAddTerminal: () => { void onCreateTerminal(menuProject.id); },
        terminalLimit: menuTerminalCount >= maxPerProject ? maxPerProject : undefined,
        onTogglePinned: () => { void onProjectFlags(menuProject.id, { pinned: !menuProject.pinned }); },
        onToggleArchived: () => { void onProjectFlags(menuProject.id, { archived: !menuProject.archived }); },
      }} />}</MotionPresence>
  </aside>;
});
