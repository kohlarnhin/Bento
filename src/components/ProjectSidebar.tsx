import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { PointerEvent as ReactPointerEvent, ReactNode, RefObject } from 'react';
import { ChevronRight, GripVertical, LayoutDashboard, Plus, RefreshCw, RotateCcw, Search, Square, Terminal, X } from 'lucide-react';
import type { PortConflict, ProjectConfig, ProjectSelection, ProjectStatus } from '../types';
import { getTerminalSnapshot, restartTerminal, subscribeTerminals, terminalKey } from '../lib/terminalRegistry';
import { cn } from '../lib/utils';
import logo from '../assets/logo.svg';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';
import { ScrollArea } from './ui/scroll-area';
import { Separator } from './ui/separator';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from './ui/collapsible';
import { Input } from './ui/input';

interface Props {
  projects: ProjectConfig[];
  visibleProjects: ProjectConfig[];
  searchQuery: string;
  searchInputRef: RefObject<HTMLInputElement>;
  onSearchChange: (query: string) => void;
  onReorder: (ids: string[]) => Promise<void>;
  reordering: boolean;
  statuses: Record<string, ProjectStatus>;
  conflicts: PortConflict[];
  selection: ProjectSelection | null;
  activeSlots: Record<string, number>;
  loading: boolean;
  onSelect: (selection: ProjectSelection) => void;
  onRefresh: () => void;
  onCreate: () => void;
  managedCount: number;
  onStopAll: () => void;
  updateFooter: ReactNode;
}

interface ProjectDrag {
  id: string;
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
  ghost?: HTMLElement;
  frame?: number;
}

const statusNames: Record<ProjectStatus, string> = {
  running: '运行中', starting: '启动中', stopped: '未运行', error: '运行异常',
};
const statusColors: Record<ProjectStatus, string> = {
  running: 'bg-success', starting: 'bg-foreground animate-pulse', stopped: 'bg-muted-foreground/35', error: 'bg-destructive',
};

function RestartTerminalButton({ projectId, slot, count, alive, opening, active, onSelect }: {
  projectId: string; slot: number; count: number; alive: boolean; opening: boolean; active: boolean; onSelect: Props['onSelect'];
}) {
  const [confirmUntil, setConfirmUntil] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    setConfirmUntil(0);
    return () => clearTimeout(timer.current);
  }, [slot]);
  const confirming = alive && confirmUntil > 0;
  const label = confirming ? '再次点击确认重启' : count > 1 ? `重启终端 ${slot}` : '重启终端';
  return (
    <ActionTooltip label={label} side="right">
      <Button
        size="icon-sm"
        variant={confirming ? 'ghost-destructive' : 'ghost'}
        className={cn('no-drag mr-1 shrink-0 opacity-0 transition-opacity group-hover/terminal:opacity-100 group-focus-within/terminal:opacity-100', (active || confirming) && 'opacity-100')}
        aria-label={label}
        title={label}
        data-sidebar-terminal-restart={projectId}
        data-confirming={confirming}
        disabled={opening}
        onClick={() => {
          onSelect({ projectId, view: 'terminal' });
          if (alive && Date.now() >= confirmUntil) {
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
        <RotateCcw data-icon="inline-start" className={cn(opening && 'animate-spin')} />
      </Button>
    </ActionTooltip>
  );
}

export function ProjectSidebar({ projects, visibleProjects, searchQuery, searchInputRef, onSearchChange, onReorder, reordering, statuses, conflicts, selection, activeSlots, loading, onSelect, onRefresh, onCreate, managedCount, onStopAll, updateFooter }: Props) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [searchFocused, setSearchFocused] = useState(false);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const groups = useRef(new Map<string, HTMLDivElement>());
  const scrollArea = useRef<HTMLDivElement>(null);
  const indicator = useRef<HTMLDivElement>(null);
  const drag = useRef<ProjectDrag | null>(null);
  const suppressClick = useRef(false);
  const searching = searchQuery.trim().length > 0;
  const canReorder = !searching && !loading && !reordering;
  const highlighted = visibleProjects.find(project => project.id === highlightedId)?.id || visibleProjects[0]?.id;
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const terminalGroups = useMemo(() => {
    const groups: Record<string, { count: number; alive: boolean; attention: boolean }> = {};
    for (const terminal of Object.values(terminals)) {
      const group = groups[terminal.projectId] ??= { count: 0, alive: false, attention: false };
      group.count++;
      group.alive ||= !!terminal.alive;
      group.attention ||= !!terminal.attention;
    }
    return groups;
  }, [terminals]);
  const running = projects.filter((project) => statuses[project.id] === 'running').length;

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
    if (current && (!canReorder || current.ids.length !== projects.length || current.ids.some((id, index) => id !== projects[index].id))) clearDrag();
  }, [projects, canReorder, clearDrag]);

  const viewport = () => scrollArea.current?.querySelector<HTMLElement>('[data-radix-scroll-area-viewport]');
  const updateTarget = (current: ProjectDrag) => {
    const area = viewport()?.getBoundingClientRect();
    const line = indicator.current;
    if (!area || !line) return;
    current.valid = current.x >= area.left && current.x <= area.right && current.y >= area.top - 24 && current.y <= area.bottom + 24;
    line.style.visibility = current.valid ? 'visible' : 'hidden';
    if (!current.valid) return;
    const remaining = projects.filter(project => project.id !== current.id).flatMap(project => {
      const element = groups.current.get(project.id);
      return element ? [{ id: project.id, rect: element.getBoundingClientRect() }] : [];
    });
    const before = remaining.find(item => current.y < item.rect.top + item.rect.height / 2);
    current.beforeId = before?.id || null;
    const top = before?.rect.top ?? remaining[remaining.length - 1]?.rect.bottom ?? area.top;
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

  const startPointer = (event: ReactPointerEvent<HTMLButtonElement>, id: string) => {
    if (event.button !== 0) return;
    suppressClick.current = false;
    if (!canReorder) return;
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      id, ids: projects.map(project => project.id), pointerId: event.pointerId, button: event.currentTarget,
      startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY,
      offsetX: 0, offsetY: 0, active: false, valid: false, beforeId: null,
    };
  };

  const movePointer = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = drag.current;
    if (!current || event.pointerId !== current.pointerId) return;
    current.x = event.clientX;
    current.y = event.clientY;
    if (!current.active) {
      if (Math.hypot(current.x - current.startX, current.y - current.startY) < 4) return;
      const group = groups.current.get(current.id);
      if (!group) { clearDrag(); return; }
      current.active = true;
      suppressClick.current = true;
      const rect = group.getBoundingClientRect();
      current.offsetX = current.startX - rect.left;
      current.offsetY = current.startY - rect.top;
      const ghost = group.cloneNode(true) as HTMLElement;
      ghost.setAttribute('aria-hidden', 'true');
      for (const node of [ghost, ...ghost.querySelectorAll<HTMLElement>('*')]) {
        node.removeAttribute('id');
        for (const attribute of [...node.attributes]) {
          if (attribute.name.startsWith('data-project') || attribute.name.startsWith('data-sidebar')) node.removeAttribute(attribute.name);
        }
        if (node instanceof HTMLButtonElement) node.tabIndex = -1;
      }
      ghost.classList.add('pointer-events-none', 'rounded-md', 'bg-sidebar', 'text-sidebar-foreground', 'shadow-xl', 'ring-1', 'ring-sidebar-border');
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
    if (current.ids.length !== projects.length || current.ids.some((id, index) => id !== projects[index].id)) { clearDrag(); return; }
    const ids = projects.filter(project => project.id !== current.id).map(project => project.id);
    const position = current.beforeId ? ids.indexOf(current.beforeId) : ids.length;
    ids.splice(position, 0, current.id);
    const changed = current.active && current.valid && ids.some((id, index) => id !== projects[index].id);
    clearDrag();
    if (changed) void onReorder(ids);
  };

  const selectOverview = (id: string) => {
    setCollapsed(previous => { const next = new Set(previous); next.delete(id); return next; });
    onSelect({ projectId: id, view: 'overview' });
  };

  return (
    <aside className="app-sidebar flex shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground data-[folder-drag=true]:ring-2 data-[folder-drag=true]:ring-inset data-[folder-drag=true]:ring-primary/40" aria-label="项目侧边栏" onKeyDownCapture={event => {
      if (event.key === 'Escape' && drag.current?.active) { event.preventDefault(); event.stopPropagation(); clearDrag(); }
    }}>
      <div data-tauri-drag-region className="h-10 shrink-0" />
      <header data-tauri-drag-region className="flex shrink-0 items-center gap-2 px-4 pb-5 pt-2">
        <img src={logo} alt="" className="pointer-events-none size-9 shrink-0" />
        <div className="pointer-events-none min-w-0 flex-1">
          <h1 className="text-base font-semibold tracking-tight text-foreground">Bento</h1>
          <p className="mt-0.5 truncate text-[11px] text-muted-foreground">{projects.length} 个项目 · {running} 个运行中</p>
        </div>
        <div className="no-drag flex shrink-0 self-start">
          <ActionTooltip label="新建项目 (⌘N)" side="bottom">
            <Button variant="ghost" size="icon-sm" aria-label="新建项目 (⌘N)" onClick={onCreate} data-create-project><Plus /></Button>
          </ActionTooltip>
          <ActionTooltip label="刷新项目状态" side="bottom">
            <Button variant="ghost" size="icon-sm" aria-label="刷新项目状态" onClick={onRefresh} className="no-drag self-start" disabled={loading}>
              <RefreshCw data-icon="inline-start" className={cn(loading && 'animate-spin')} />
            </Button>
          </ActionTooltip>
        </div>
      </header>
      <Separator />
      <div className="relative mx-3 mb-1 mt-3 shrink-0">
        <Search className="pointer-events-none absolute left-2.5 top-2 size-3.5 text-muted-foreground" />
        <Input ref={searchInputRef} type="search" placeholder="搜索项目…" aria-label="搜索项目" title="搜索项目 (⌘F)" aria-keyshortcuts="Meta+f" aria-controls="project-list" data-project-search
          className="h-8 bg-background/70 pl-8 pr-8 text-xs [&::-webkit-search-cancel-button]:hidden"
          value={searchQuery}
          onFocus={() => setSearchFocused(true)} onBlur={() => setSearchFocused(false)}
          onChange={event => { onSearchChange(event.target.value); setHighlightedId(null); }}
          onKeyDown={event => {
            if (event.nativeEvent.isComposing || event.keyCode === 229) return;
            if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
              event.preventDefault();
              const index = Math.max(0, visibleProjects.findIndex(project => project.id === highlighted));
              const next = visibleProjects[Math.max(0, Math.min(visibleProjects.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
              if (next) {
                setHighlightedId(next.id);
                groups.current.get(next.id)?.querySelector('[data-project-row]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
              }
            } else if (event.key === 'Enter') {
              event.preventDefault();
              if (highlighted) selectOverview(highlighted);
            } else if (event.key === 'Escape') {
              event.preventDefault();
              event.stopPropagation();
              if (searchQuery) { onSearchChange(''); setHighlightedId(null); }
              else event.currentTarget.blur();
            }
          }} />
        {searchQuery && <Button variant="ghost" size="icon-sm" className="absolute right-0.5 top-0.5 size-7 text-muted-foreground" aria-label="清除搜索" onClick={() => {
          onSearchChange(''); setHighlightedId(null); searchInputRef.current?.focus();
        }}><X className="size-3.5" /></Button>}
      </div>
      <ScrollArea ref={scrollArea} className="min-h-0 flex-1 px-3 py-3">
        <nav id="project-list" aria-label="项目列表" className="flex flex-col gap-1">
          <div className="mb-2 flex h-5 items-center justify-between px-2 text-[11px] font-medium text-muted-foreground">
            <span>项目</span><span className="tabular-nums" data-project-count>{searching ? `${visibleProjects.length} / ${projects.length}` : projects.length}</span>
          </div>
          {searching && <p className="mb-2 px-2 text-[10px] leading-relaxed text-muted-foreground" data-reorder-search-hint>清除搜索后可拖动排序</p>}
          {searching && !visibleProjects.length && <p role="status" className="px-2 py-6 text-xs leading-relaxed text-muted-foreground break-all" data-project-search-empty>没有匹配「{searchQuery.trim()}」的项目</p>}
          {visibleProjects.map((project, index) => {
            const selected = selection?.projectId === project.id;
            const terminalGroup = terminalGroups[project.id];
            const activeSlot = activeSlots[project.id] ?? 1;
            const activeTerminal = terminals[terminalKey(project.id, activeSlot)];
            const expanded = !collapsed.has(project.id);
            const conflict = conflicts.some((item) => item.projectIds.includes(project.id));
            const status = statuses[project.id] || 'stopped';
            const name = project.displayName || project.name;
            return (
              <Collapsible key={project.id} ref={node => { if (node) groups.current.set(project.id, node); else groups.current.delete(project.id); }} open={expanded} onOpenChange={(open) => setCollapsed((previous) => {
                const next = new Set(previous);
                if (open) next.delete(project.id); else next.add(project.id);
                return next;
              })} data-project-id={project.id} data-project-dragging={draggingId === project.id || undefined} className={cn(draggingId === project.id && 'opacity-40')}>
                <div className={cn('group/project flex items-center rounded-md transition-colors hover:bg-sidebar-accent', selected && 'text-sidebar-accent-foreground', searchFocused && highlighted === project.id && 'bg-sidebar-accent ring-1 ring-inset ring-sidebar-primary/40')}>
                  <CollapsibleTrigger asChild>
                    <Button size="icon-sm" variant="ghost" aria-label={`${expanded ? '收起' : '展开'} ${name}`} className="shrink-0">
                      <ChevronRight className={cn('transition-transform', expanded && 'rotate-90')} />
                    </Button>
                  </CollapsibleTrigger>
                  <button className={cn('flex min-w-0 flex-1 items-center gap-2 rounded-md py-2 pr-2 text-left text-xs font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-sidebar-ring/50', canReorder && 'touch-none cursor-grab active:cursor-grabbing')}
                    onPointerDown={event => startPointer(event, project.id)} onPointerMove={movePointer} onPointerUp={finishPointer}
                    onPointerCancel={clearDrag} onLostPointerCapture={event => { if (drag.current?.pointerId === event.pointerId) clearDrag(); }}
                    onKeyDown={event => {
                      if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
                      event.preventDefault();
                      if (!canReorder) return;
                      const position = projects.findIndex(item => item.id === project.id);
                      const target = position + (event.key === 'ArrowDown' ? 1 : -1);
                      if (target < 0 || target >= projects.length) return;
                      const ids = projects.map(item => item.id);
                      [ids[position], ids[target]] = [ids[target], ids[position]];
                      void onReorder(ids);
                      requestAnimationFrame(() => {
                        const row = groups.current.get(project.id)?.querySelector<HTMLButtonElement>('[data-project-row]');
                        row?.focus({ preventScroll: true });
                        row?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
                      });
                    }}
                    onClick={event => {
                      if (suppressClick.current && event.detail !== 0) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; return; }
                      selectOverview(project.id);
                    }}
                    title={`${name}${index < 9 ? ` · ⌘${index + 1}` : ''}\n${searching ? '清除搜索后可拖动排序' : '拖动排序 · ⌥↑ / ⌥↓'}`}
                    data-project-row={project.id} data-search-highlighted={searchFocused && highlighted === project.id || undefined} data-reorder-draggable={canReorder || undefined}>
                    <span className={cn('size-1.5 shrink-0 rounded-full', conflict ? 'bg-warning' : statusColors[status])} aria-label={conflict ? '端口冲突' : statusNames[status]} />
                    <span className="truncate">{name}</span>
                    {!expanded && terminalGroup?.attention && <span className="ml-auto size-1.5 shrink-0 rounded-full bg-attention" aria-label="终端有新动静" title="终端有新动静" data-terminal-attention={project.id} />}
                    {!searching && <GripVertical aria-hidden="true" className={cn('size-3 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/project:opacity-60 group-focus-within/project:opacity-60', expanded || !terminalGroup?.attention ? 'ml-auto' : '')} />}
                  </button>
                </div>
                <CollapsibleContent>
                  <div className="mb-2 ml-3.5 mt-1 flex flex-col gap-1 border-l border-sidebar-border pl-3">
                    {(['overview', 'terminal'] as const).map((view) => {
                      const active = selected && selection.view === view;
                      const Icon = view === 'overview' ? LayoutDashboard : Terminal;
                      return (
                        <div key={view} className={cn('group/terminal flex items-center rounded-md transition-colors hover:bg-sidebar-accent/70', active && 'bg-sidebar-accent text-sidebar-accent-foreground')}>
                          <button data-project-view={view} data-project-target={project.id} aria-current={active ? 'page' : undefined} onClick={() => onSelect({ projectId: project.id, view })}
                            className={cn('flex min-w-0 flex-1 items-center gap-2 rounded-md px-2.5 py-2 text-left text-xs outline-none focus-visible:ring-[3px] focus-visible:ring-sidebar-ring/50', active ? 'font-medium' : 'text-muted-foreground')}>
                            <Icon className="size-3.5 shrink-0" />
                            <span>{view === 'overview' ? '概览' : '终端'}</span>
                            {view === 'terminal' && terminalGroup?.count > 1 && <span className="text-[10px] tabular-nums text-muted-foreground" data-project-terminal-count>{terminalGroup.count}</span>}
                            {view === 'terminal' && (terminalGroup?.attention ? <span className="ml-auto size-1.5 shrink-0 rounded-full bg-attention" aria-label="终端有新动静" title="终端有新动静" data-terminal-attention={project.id} /> : terminalGroup?.alive && <span className="ml-auto size-1.5 rounded-full bg-success" aria-label="终端运行中" />)}
                          </button>
                          {view === 'terminal' && <RestartTerminalButton projectId={project.id} slot={activeSlot} count={terminalGroup?.count ?? 1} alive={!!activeTerminal?.alive} opening={!!activeTerminal?.opening} active={!!active} onSelect={onSelect} />}
                        </div>
                      );
                    })}
                  </div>
                </CollapsibleContent>
              </Collapsible>
            );
          })}
        </nav>
      </ScrollArea>
      {managedCount > 0 && <footer className="shrink-0 border-t border-sidebar-border px-3 py-3" data-managed-footer>
        <p className="text-[11px] leading-relaxed text-muted-foreground">Bento 启动的项目：{managedCount} 个运行中</p>
        <Button variant="outline" size="sm" className="mt-2 w-full text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={onStopAll} data-stop-all><Square data-icon="inline-start" />全部停止</Button>
      </footer>}
      {updateFooter}
      <div ref={indicator} hidden={!draggingId} aria-hidden="true" data-reorder-indicator className="pointer-events-none fixed z-[70] h-0.5 rounded-full bg-primary" />
    </aside>
  );
}
