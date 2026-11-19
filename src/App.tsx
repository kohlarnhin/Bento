import { useEffect, useState, useCallback, useRef, useMemo, useSyncExternalStore, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { DragDropEvent } from '@tauri-apps/api/webview';
import { FolderPlus } from 'lucide-react';
import { toast } from 'sonner';
import { TooltipProvider } from './components/ui/tooltip';
import { ProjectCard } from './components/ProjectCard';
import { TerminalLogsModal } from './components/TerminalLogs';
import { ProjectSidebar } from './components/ProjectSidebar';
import { ProjectFocusRail } from './components/ProjectFocusRail';
import { TerminalWorkspace, type TerminalWorkspaceControls } from './components/TerminalWorkspace';
import { ProjectFormDialog } from './components/ProjectFormDialog';
import { ProjectDeleteDialog } from './components/ProjectDeleteDialog';
import { ExternalProcessDialog } from './components/ExternalProcessDialog';
import { StopAllProjectsDialog } from './components/StopAllProjectsDialog';
import { UpdateDialog } from './components/UpdateDialog';
import { useUpdater } from './hooks/useUpdater';
import { useSettings } from './hooks/useSettings';
import { SettingsDialog } from './components/SettingsDialog';
import { QuickSwitcher } from './components/QuickSwitcher';
import { GroupDialog } from './components/GroupDialog';
import Button from './components/ui/button';
import { Toaster } from './components/ui/sonner';
import { Empty, EmptyTitle, EmptyDescription } from './components/ui/empty';
import { createTerminal, getProjectTerminalSlots, getTerminalSnapshot, initTerminalRegistry, subscribeTerminals, terminalKey } from './lib/terminalRegistry';
import { projectLogs } from './lib/useProjectLogs';
import { recordRender } from './lib/renderMetrics';
import { DEFAULT_TERMINAL_FONT_SIZE, TERMINAL_THEME } from './lib/terminalAppearance';
import { clampTerminalFontSize, firstOpenedTerminalSlot, terminalsNeedingAttention } from './lib/terminalUsability';
import { matchesSidebarFilter, sidebarNavigation, sidebarSections, type SidebarFilter } from './lib/sidebarModel';
import { useProjectGitInfos } from './lib/useGitInfo';
import { sameServiceStatuses } from './lib/projectServices';
import {
  ProjectConfig,
  ProjectGroup,
  PortConflict,
  ProjectsResponse,
  ProjectStatus,
  ProjectRuntimeStatus,
  ProjectSelection,
  ProjectDirInspection,
  PortOwner,
  DashboardConflict,
} from './types';
import './components/terminal.css';

let sidebarDropContext: {
  isDialogOpen: () => boolean;
  openCreate: (inspection: ProjectDirInspection) => void;
  showHover: (name: string | null) => void;
} | undefined;
let sidebarDragPaths: string[] = [];
let sidebarDropPending = false;

export async function handleSidebarDragDrop(event: DragDropEvent) {
  const sidebar = document.querySelector<HTMLElement>('aside.app-sidebar');
  if (!sidebar) return;
  const context = sidebarDropContext;
  if (event.type === 'enter') sidebarDragPaths = event.paths;
  if (!context || context.isDialogOpen() || event.type === 'leave') {
    context?.showHover(null);
    sidebarDragPaths = [];
    return;
  }
  const rect = sidebar.getBoundingClientRect();
  const x = event.position.x / window.devicePixelRatio;
  const y = event.position.y / window.devicePixelRatio;
  if (x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom) {
    context.showHover(null);
    if (event.type === 'drop') sidebarDragPaths = [];
    return;
  }
  if (event.type === 'drop' && !event.paths.length) { context.showHover(null); sidebarDragPaths = []; return; }
  const paths = event.type === 'drop' ? event.paths : sidebarDragPaths;
  const name = paths[0]?.split(/[/\\]/u).filter(Boolean).at(-1) || '项目文件夹';
  if (event.type === 'enter' || event.type === 'over') context.showHover(name);
  else if (event.type === 'drop' && event.paths[0]) {
    context.showHover(null);
    sidebarDragPaths = [];
    if (sidebarDropPending) return;
    sidebarDropPending = true;
    try {
      if (paths.length > 1) toast.info(`一次仅处理首项，已选择「${name}」（共 ${paths.length} 项）`);
      const inspection = await invoke<ProjectDirInspection>('inspect_project_dir', { path: event.paths[0], excludeId: null });
      if (context !== sidebarDropContext || context.isDialogOpen()) return;
      if (!inspection.exists) toast.error(`「${name}」路径不存在，请重新拖入项目文件夹`);
      else if (!inspection.isDir) toast.error(`「${name}」不是文件夹，请拖入项目文件夹`);
      else if (inspection.existingProject) toast.info(`此文件夹已添加为项目「${inspection.existingProject.name}」，请在侧边栏打开`);
      else context.openCreate(inspection);
    } catch (error) {
      toast.error(`检查项目目录失败：${error}`);
    } finally { sidebarDropPending = false; }
  }
}

interface LogPayload {
  projectId: string;
  text: string;
  stream: 'stdout' | 'stderr' | 'system';
}

interface LogBatchPayload {
  projectId: string;
  entries: Pick<LogPayload, 'text' | 'stream'>[];
}

type StatusPayload = ProjectRuntimeStatus;

export function App() {
  recordRender('app');
  const updater = useUpdater();
  const settings = useSettings();
  const terminalControls = useRef<TerminalWorkspaceControls>(null);
  const [focusMode, setFocusMode] = useState(false);
  const toggleFocusMode = useCallback(() => {
    setFocusMode(value => !value);
    terminalControls.current?.focusInput();
  }, []);
  const toggleSplit = useCallback(() => terminalControls.current?.toggleSplit(), []);
  const changeFontSize = settings.changeFontSize;
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [folderDropName, setFolderDropName] = useState<string | null>(null);
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const terminalCount = Object.values(terminals).filter(terminal => terminal.alive).length;
  const [projects, setProjects] = useState<ProjectConfig[]>([]);
  const [groups, setGroups] = useState<ProjectGroup[]>([]);
  const [filter, setFilter] = useState<SidebarFilter>('all');
  const [quickOpen, setQuickOpen] = useState(false);
  const quickOpenRef = useRef(quickOpen);
  quickOpenRef.current = quickOpen;
  const [groupForm, setGroupForm] = useState<{ action: 'create' | 'rename' | 'delete'; group?: ProjectGroup } | null>(null);
  const gitInfos = useProjectGitInfos(projects);
  const attentionProjects = useMemo(() => new Set(Object.values(terminals).filter(terminal => terminal.attention).map(terminal => terminal.projectId)), [terminals]);
  const changedProjects = useMemo(() => new Set(projects.filter(project => (gitInfos[project.id]?.changes ?? 0) > 0).map(project => project.id)), [projects, gitInfos]);
  const [conflicts, setConflicts] = useState<PortConflict[]>([]);
  const [runtimes, setRuntimes] = useState<Record<string, ProjectRuntimeStatus>>({});
  const projectStatuses: Record<string, ProjectStatus> = useMemo(() => Object.fromEntries(Object.entries(runtimes).map(([id, runtime]) => [id, runtime.status])), [runtimes]);
  const applyRuntime = useCallback((updates: ProjectRuntimeStatus[]) => {
    setRuntimes(previous => {
      let changed = false;
      const next = { ...previous };
      for (const runtime of updates) {
        const current = previous[runtime.projectId];
        if (current && runtime.revision < current.revision) continue;
        if (current && (Object.keys(runtime) as (keyof ProjectRuntimeStatus)[]).every(key =>
          key === 'services' ? sameServiceStatuses(current.services, runtime.services) : current[key] === runtime[key])) continue;
        next[runtime.projectId] = runtime;
        changed = true;
      }
      return changed ? next : previous;
    });
  }, []);
  const refreshRuntime = useCallback(async () => {
    applyRuntime(await invoke<ProjectRuntimeStatus[]>('get_projects_runtime_status'));
  }, [applyRuntime]);
  const { enqueue: enqueueLogs, flush: flushLogs, clear: clearLogs } = projectLogs;
  const [activeLogProject, setActiveLogProject] = useState<ProjectConfig | null>(null);
  const [activeLogTarget, setActiveLogTarget] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [{ selection, activeSlots }, setProjectNavigation] = useState<{
    selection: ProjectSelection | null;
    activeSlots: Record<string, number>;
  }>({ selection: null, activeSlots: {} });
  const setSelection = useCallback((selection: ProjectSelection) => {
    setProjectNavigation(previous => ({ ...previous, selection }));
  }, []);
  const selectTerminalSlot = useCallback((projectId: string, slot: number) => {
    setProjectNavigation(previous => ({
      selection: { projectId, view: 'terminal' },
      activeSlots: { ...previous.activeSlots, [projectId]: slot },
    }));
  }, []);
  const selectProject = useCallback((projectId: string) => {
    setActiveLogProject(null);
    const slot = firstOpenedTerminalSlot(projectId, Object.values(getTerminalSnapshot()));
    if (slot === undefined) setSelection({ projectId, view: 'overview' });
    else selectTerminalSlot(projectId, slot);
  }, [selectTerminalSlot, setSelection]);
  useEffect(() => {
    setProjectNavigation(navigation => {
      const previous = navigation.activeSlots;
      let next = previous;
      let selected = navigation.selection;
      for (const [projectId, slot] of Object.entries(previous)) {
        if (slot === 1 || terminals[terminalKey(projectId, slot)]) continue;
        const slots = getProjectTerminalSlots(projectId, terminals).filter(candidate => terminals[terminalKey(projectId, candidate)]?.sessionId);
        const replacement = slots.filter(candidate => candidate < slot).at(-1) ?? slots.find(candidate => candidate > slot);
        if (next === previous) next = { ...previous };
        if (replacement !== undefined) next[projectId] = replacement;
        else {
          delete next[projectId];
          if (selected?.projectId === projectId && selected.view === 'terminal') selected = { projectId, view: 'overview' };
        }
      }
      return next === previous ? navigation : { selection: selected, activeSlots: next };
    });
  }, [terminals]);
  const onTerminalClosed = useCallback((projectId: string, slot: number) => {
    const replacement = firstOpenedTerminalSlot(projectId, Object.values(getTerminalSnapshot()));
    setProjectNavigation(previous => {
      if (previous.selection?.projectId !== projectId || previous.selection.view !== 'terminal' || (previous.activeSlots[projectId] ?? 1) !== slot) return previous;
      return replacement === undefined ? { ...previous, selection: { projectId, view: 'overview' } } :
        { ...previous, activeSlots: { ...previous.activeSlots, [projectId]: replacement } };
    });
  }, []);
  const [focusedTerminal, setFocusedTerminal] = useState<{ projectId: string; slot: number }>();
  const onFocusedTerminal = useCallback((projectId: string, slot: number) => {
    setFocusedTerminal(previous => previous?.projectId === projectId && previous.slot === slot ? previous : { projectId, slot });
  }, []);
  const watchedProjectId = selection?.view === 'terminal' && !quickOpen ? selection.projectId : null;
  const watchedSlot = watchedProjectId ? (focusedTerminal?.projectId === watchedProjectId ? focusedTerminal.slot : activeSlots[watchedProjectId] ?? 1) : null;
  const reportedTerminal = useRef<string | null>();
  const focusRequests = useRef(Promise.resolve());
  useEffect(() => {
    const report = (projectId: string | null, slot: number | null) => {
      const key = projectId && slot ? terminalKey(projectId, slot) : null;
      if (reportedTerminal.current === key) return;
      reportedTerminal.current = key;
      focusRequests.current = focusRequests.current.catch(() => {}).then(() =>
        invoke<void>('terminal_focus', { projectId, slot }),
      );
      void focusRequests.current.catch(error => toast.error(`更新终端关注状态失败：${error}`));
    };
    const update = () => {
      if (document.visibilityState === 'visible' && document.hasFocus()) report(watchedProjectId, watchedSlot);
      else report(null, null);
    };
    const blur = () => report(null, null);
    update();
    window.addEventListener('focus', update);
    window.addEventListener('blur', blur);
    window.addEventListener('pagehide', blur);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.removeEventListener('focus', update);
      window.removeEventListener('blur', blur);
      window.removeEventListener('pagehide', blur);
      document.removeEventListener('visibilitychange', update);
    };
  }, [watchedProjectId, watchedSlot]);
  const [searchQuery, setSearchQuery] = useState('');
  const searchInput = useRef<HTMLInputElement>(null);
  const searchText = searchQuery.trim().toLowerCase();
  const visibleProjects = useMemo(() => sidebarSections(projects.filter(project =>
    matchesSidebarFilter(project, filter, projectStatuses, attentionProjects, changedProjects) && (!searchText ||
    [project.displayName, project.name, project.path, project.port?.toString()].some(value => value?.toLowerCase().includes(searchText)))), groups).flatMap(section => section.projects),
    [projects, groups, searchText, filter, projectStatuses, attentionProjects, changedProjects]);
  const shortcutProjects = useMemo(() => visibleProjects.filter(project => !project.archived), [visibleProjects]);
  const navigation = useRef({ projects: shortcutProjects, view: selection?.view || 'overview', selection, activeSlots });
  navigation.current = { projects: shortcutProjects, view: selection?.view || 'overview', selection, activeSlots };
  const [reordering, setReordering] = useState(false);
  const reorderPending = useRef(false);
  const [projectForm, setProjectForm] = useState<{ project?: ProjectConfig; inspection?: ProjectDirInspection; focusPort?: DashboardConflict } | null>(null);
  const [deleteProject, setDeleteProject] = useState<ProjectConfig | null>(null);
  const [externalStop, setExternalStop] = useState<{ project: ProjectConfig; owners: PortOwner[]; restart: boolean } | null>(null);
  const [stopAllOpen, setStopAllOpen] = useState(false);
  const managedProjects = projects.filter(project => runtimes[project.id]?.isManaged && ['starting', 'running'].includes(runtimes[project.id].status));
  const externalRequest = useRef(false);
  const dialogOpen = useRef(false);
  dialogOpen.current = !!(projectForm || deleteProject || activeLogProject || externalStop || stopAllOpen || settingsOpen || quickOpen || groupForm || externalRequest.current || updater.isModalOpen);
  const openQuickSwitcher = useCallback(() => {
    if (dialogOpen.current) return;
    terminalControls.current?.closeFind();
    dialogOpen.current = true;
    setQuickOpen(true);
  }, []);
  const closeQuickSwitcher = useCallback(() => { setQuickOpen(false); quickOpenRef.current = false; dialogOpen.current = false; }, []);
  const openSettings = useCallback(() => {
    if (dialogOpen.current) return;
    dialogOpen.current = true;
    setSettingsOpen(true);
  }, []);
  const openCreate = useCallback((inspection?: ProjectDirInspection) => {
    if (dialogOpen.current) return;
    setFolderDropName(null);
    dialogOpen.current = true;
    setProjectForm({ inspection });
  }, []);

  const creatingTerminals = useRef(new Set<string>());
  const openTerminal = useCallback(async (projectId: string) => {
    if (dialogOpen.current || creatingTerminals.current.has(projectId)) return;
    creatingTerminals.current.add(projectId);
    try {
      const slot = await createTerminal(projectId);
      selectTerminalSlot(projectId, slot);
    } catch (error) {
      toast.error(String(error));
    } finally {
      creatingTerminals.current.delete(projectId);
    }
  }, [selectTerminalSlot]);

  useEffect(() => {
    const captured = new Set<string>();
    let composing = false;
    const startComposition = () => { composing = true; };
    const endComposition = () => { composing = false; };
    const onShortcut = (event: KeyboardEvent) => {
      const physicalKey = event.code || event.key;
      if (event.type !== 'keydown' && captured.has(physicalKey)) {
        event.preventDefault();
        event.stopPropagation();
        if (event.type === 'keyup') captured.delete(physicalKey);
        return;
      }
      if (composing || event.isComposing || event.keyCode === 229) return;
      if (quickOpenRef.current) {
        // Panel actions may close the dialog on keydown; swallow their later
        // keypress/keyup events before focus returns to an existing terminal.
        if (event.type === 'keydown' && (['Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)
          || (event.metaKey && ['t', 'n', 'd'].includes(event.key.toLowerCase())))) captured.add(physicalKey);
        return;
      }
      const inTerminal = navigation.current.view === 'terminal';
      const finding = inTerminal && terminalControls.current?.isFindOpen();
      if (finding && event.key === 'Escape' && !event.metaKey && !event.ctrlKey && !event.altKey
          && !dialogOpen.current && !document.querySelector('[role="dialog"][data-state="open"]')) {
        event.preventDefault();
        event.stopPropagation();
        if (event.type === 'keydown') {
          captured.add(physicalKey);
          terminalControls.current?.closeFind();
        }
        return;
      }
      if (!event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.keyCode === 229) return;
      const key = event.key.toLowerCase();
      const focusShortcut = inTerminal && event.shiftKey && key === 'enter';
      const splitShortcut = inTerminal && !event.shiftKey && key === 'd';
      const cycle = event.shiftKey && ['BracketLeft', 'BracketRight'].includes(event.code) && inTerminal;
      const zoomIn = inTerminal && (event.code === 'Equal' || key === '=' || key === '+');
      const zoomOut = inTerminal && !event.shiftKey && (event.code === 'Minus' || key === '-');
      const zoomReset = inTerminal && !event.shiftKey && (event.code === 'Digit0' || key === '0');
      const jump = event.shiftKey && (event.code === 'KeyU' || key === 'u');
      const findNext = finding && key === 'g';
      if (!focusShortcut && !splitShortcut && !cycle && !zoomIn && !zoomOut && !zoomReset && !jump && !findNext
          && (event.shiftKey || (key !== 'p' && key !== 't' && key !== 'n' && key !== 'f' && key !== ',' && !/^[1-9]$/.test(key)))) return;
      event.preventDefault();
      event.stopPropagation();
      // xterm also handles keyup; swallowing the complete shortcut keeps focus
      // in search and prevents a later event from reaching the old terminal.
      if (event.type !== 'keydown') return;
      captured.add(physicalKey);
      if (dialogOpen.current || document.querySelector('[role="dialog"][data-state="open"]')) return;
      if (key === 'p' && !event.repeat) openQuickSwitcher();
      else if (focusShortcut) { if (!event.repeat) toggleFocusMode(); }
      else if (splitShortcut) { if (!event.repeat) toggleSplit(); }
      else if (zoomIn || zoomOut || zoomReset) {
        void changeFontSize(current => zoomReset ? DEFAULT_TERMINAL_FONT_SIZE : clampTerminalFontSize(current + (zoomIn ? 1 : -1)));
      } else if (jump) {
        const target = terminalsNeedingAttention(Object.values(getTerminalSnapshot()))[0];
        if (!target) toast('没有等待处理的终端');
        else {
          if (!navigation.current.projects.some(project => project.id === target.projectId)) { setSearchQuery(''); setFilter('all'); }
          selectTerminalSlot(target.projectId, target.slot);
        }
      } else if (findNext) terminalControls.current?.navigateFind(event.shiftKey);
      else if (cycle) {
        const projectId = navigation.current.selection?.projectId;
        if (!projectId) return;
        const slots = getProjectTerminalSlots(projectId);
        const index = slots.indexOf(navigation.current.activeSlots[projectId] ?? 1);
        const direction = event.code === 'BracketLeft' ? -1 : 1;
        selectTerminalSlot(projectId, slots[(index + direction + slots.length) % slots.length]);
      } else if (key === 't') {
        const projectId = navigation.current.selection?.projectId;
        if (projectId && !event.repeat) void openTerminal(projectId);
      } else if (key === ',') openSettings();
      else if (key === 'n' && !event.repeat) openCreate();
      else if (key === 'f') {
        if (inTerminal) terminalControls.current?.openFind();
        else { searchInput.current?.focus(); searchInput.current?.select(); }
      }
      else {
        const project = navigation.current.projects[Number(key) - 1];
        if (project) selectProject(project.id);
      }
    };
    for (const type of ['keydown', 'keypress', 'keyup'] as const) window.addEventListener(type, onShortcut, true);
    window.addEventListener('compositionstart', startComposition, true);
    window.addEventListener('compositionend', endComposition, true);
    sidebarDropContext = { isDialogOpen: () => dialogOpen.current, openCreate, showHover: setFolderDropName };
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => { void handleSidebarDragDrop(payload); });
    return () => {
      for (const type of ['keydown', 'keypress', 'keyup'] as const) window.removeEventListener(type, onShortcut, true);
      window.removeEventListener('compositionstart', startComposition, true);
      window.removeEventListener('compositionend', endComposition, true);
      sidebarDropContext = undefined;
      sidebarDragPaths = [];
      void unlisten.then((stop) => stop());
    };
  }, [openCreate, openSettings, openQuickSwitcher, openTerminal, selectProject, selectTerminalSlot, changeFontSize, toggleFocusMode, toggleSplit]);

  useEffect(() => {
    const unlisten = listen('bento-open-settings', openSettings);
    return () => { void unlisten.then(stop => stop()); };
  }, [openSettings]);

  useEffect(() => {
    if (updater.isModalOpen) setSettingsOpen(false);
  }, [updater.isModalOpen]);

  useEffect(() => {
    const unlisten = listen<{ projectId: string }>('apphub-select-project', ({ payload }) => {
      if (!navigation.current.projects.some(project => project.id === payload.projectId)) { setSearchQuery(''); setFilter('all'); }
      selectProject(payload.projectId);
    });
    return () => { void unlisten.then(stop => stop()); };
  }, [selectProject]);

  const showToast = useCallback((msg: string) => {
    toast.error(msg);
  }, []);

  // Fetch projects and current status
  const loadProjects = useCallback(async () => {
    try {
      setLoading(true);
      const [res] = await Promise.all([
        invoke<ProjectsResponse>('get_projects'),
        initTerminalRegistry(showToast).catch(() => {}),
      ]);
      setProjects(res.projects);
      setGroups(res.groups);
      setProjectsLoaded(true);
      setConflicts(res.conflicts);
      const first = sidebarNavigation(res.projects, res.groups)[0] ?? res.projects[0];
      const slot = first && firstOpenedTerminalSlot(first.id, Object.values(getTerminalSnapshot()));
      setProjectNavigation(previous => {
        if (res.projects.some(project => project.id === previous.selection?.projectId)) return previous;
        return {
          selection: first ? { projectId: first.id, view: slot === undefined ? 'overview' : 'terminal' } : null,
          activeSlots: first && slot !== undefined ? { ...previous.activeSlots, [first.id]: slot } : previous.activeSlots,
        };
      });

      // Check port in-use / runtime status
      const runtimeStatuses = await invoke<ProjectRuntimeStatus[]>('get_projects_runtime_status');
      applyRuntime(runtimeStatuses);
    } catch (err) {
      console.error('加载项目失败:', err);
      showToast(`加载项目失败: ${err}`);
    } finally {
      setLoading(false);
    }
  }, [showToast, applyRuntime]);

  const reorderProjects = useCallback(async (ids: string[], move?: { id: string; groupId: string | null }) => {
    if (reorderPending.current) return;
    reorderPending.current = true;
    setReordering(true);
    const previous = projects;
    const byId = new Map(projects.map(project => [project.id, project]));
    setProjects(ids.map(id => { const project = byId.get(id)!; return move?.id === id ? { ...project, groupId: move.groupId } : project; }));
    try {
      await invoke(move ? 'move_project' : 'reorder_projects', { ids, ...move });
      await loadProjects();
    } catch (error) {
      setProjects(previous);
      toast.error(`排序失败：${error}`);
      await loadProjects();
    } finally {
      reorderPending.current = false;
      setReordering(false);
    }
  }, [projects, loadProjects]);

  const addGroup = useCallback(async (name: string) => {
    const group = await invoke<ProjectGroup>('add_group', { name });
    setGroups(previous => [...previous, group]);
    return group;
  }, []);
  const updateGroup = useCallback(async (id: string, changes: { name?: string; collapsed?: boolean }) => {
    await invoke('update_group', { id, ...changes });
    setGroups(previous => previous.map(group => group.id === id ? { ...group, ...changes } : group));
  }, []);
  const reorderGroups = useCallback(async (ids: string[]) => {
    if (reorderPending.current) return;
    reorderPending.current = true;
    setReordering(true);
    try {
      await invoke('reorder_groups', { ids });
      setGroups(previous => ids.map((id, sortOrder) => ({ ...previous.find(group => group.id === id)!, sortOrder })));
    } catch (error) { toast.error(`分组排序失败：${error}`); await loadProjects(); }
    finally { reorderPending.current = false; setReordering(false); }
  }, [loadProjects]);
  const setProjectFlags = useCallback(async (id: string, flags: { pinned?: boolean; archived?: boolean }) => {
    try {
      await invoke('set_project_flags', { id, ...flags });
      setProjects(previous => previous.map(project => project.id === id ? { ...project, ...flags } : project));
    } catch (error) { toast.error(`保存项目状态失败：${error}`); }
  }, []);

  // Set up event listeners and periodic status polling
  useEffect(() => {
    loadProjects();

    // Periodic port status check (detects externally started/stopped projects)
    const poll = async () => {
      try {
        await refreshRuntime();
      } catch (err) {
        // silent fail in polling
      }
    };
    let interval: ReturnType<typeof setInterval> | undefined;
    const onVisibilityChange = () => {
      clearInterval(interval);
      interval = undefined;
      if (document.visibilityState === 'visible') {
        void poll();
        interval = setInterval(poll, 2500);
      }
    };
    if (document.visibilityState === 'visible') interval = setInterval(poll, 2500);
    document.addEventListener('visibilitychange', onVisibilityChange);

    const unlistenLogPromise = listen<LogPayload>('project-log', (event) => {
      const payload = event.payload;
      enqueueLogs(payload.projectId, [payload]);
    });
    const unlistenBatchPromise = listen<LogBatchPayload>('project-log-batch', ({ payload }) => {
      enqueueLogs(payload.projectId, payload.entries);
    });
    const unlistenRunPromise = listen<string>('project-run-started', ({ payload }) => clearLogs(payload));

    const unlistenStatusPromise = listen<StatusPayload>('project-status', (event) => {
      applyRuntime([event.payload]);
    });

    return () => {
      clearInterval(interval);
      document.removeEventListener('visibilitychange', onVisibilityChange);
      unlistenLogPromise.then((unlisten) => unlisten());
      unlistenBatchPromise.then((unlisten) => unlisten());
      unlistenRunPromise.then((unlisten) => unlisten());
      unlistenStatusPromise.then((unlisten) => unlisten());
    };
  }, [loadProjects, applyRuntime, refreshRuntime, enqueueLogs, clearLogs]);

  // Actions
  const handleStart = async (id: string) => {
    try {
      await invoke('start_project', { id });
    } catch (err) {
      console.error('启动失败:', err);
      showToast(`启动失败: ${err}`);
    }
  };

  const handleRunCommand = async (id: string, command: string) => {
    if (dialogOpen.current) return;
    try {
      const result = await invoke<{ slot: number }>('terminal_run', { projectId: id, slot: activeSlots[id] ?? 1, command });
      selectTerminalSlot(id, result.slot);
    } catch (error) {
      toast.error(String(error));
    }
  };

  const stopOrRestart = async (id: string, restart: boolean) => {
    if (dialogOpen.current) return;
    try {
      const statuses = await invoke<ProjectRuntimeStatus[]>('get_projects_runtime_status');
      applyRuntime(statuses);
      const runtime = statuses.find(item => item.projectId === id);
      if (runtime?.external) {
        externalRequest.current = true;
        dialogOpen.current = true;
        const project = projects.find(item => item.id === id);
        const owners = await invoke<PortOwner[]>('get_port_owners', { id });
        if (project && owners.length) setExternalStop({ project, owners, restart });
        else { dialogOpen.current = false; await refreshRuntime(); }
      } else {
        await invoke(restart ? 'restart_project' : 'stop_project', { id });
        await refreshRuntime();
      }
    } catch (err) {
      showToast(`${restart ? '重启' : '停止'}失败: ${err}`);
      dialogOpen.current = false;
    } finally {
      externalRequest.current = false;
    }
  };
  const handleStop = (id: string) => stopOrRestart(id, false);
  const handleRestart = (id: string) => stopOrRestart(id, true);

  const handleOpenBrowser = async (port: number) => {
    try {
      await invoke('open_url', { url: `http://localhost:${port}` });
    } catch (err) {
      showToast(`打开网页失败 · ${err}`);
    }
  };

  const handleRevealPath = async (path: string) => {
    try {
      await invoke('reveal_path', { path });
    } catch (err) {
      console.error('定位路径失败:', err);
      showToast(`定位路径失败: ${err}`);
    }
  };

  const handleClearLogs = (id: string) => {
    clearLogs(id);
  };

  const selectedProject = projects.find((project) => project.id === selection?.projectId);
  const selectedConflicts = conflicts.filter(conflict => selectedProject && conflict.projectIds.includes(selectedProject.id));
  const setDefaultService = async (serviceId: string) => {
    if (!selectedProject) return;
    try {
      await invoke('set_default_service', { id: selectedProject.id, serviceId });
      await loadProjects();
    } catch (error) {
      showToast(`设置默认服务失败：${error}`);
    }
  };
  const selectView = useCallback((next: ProjectSelection) => {
    setActiveLogProject(null);
    setSelection(next);
  }, [setSelection]);
  const selectSidebarTerminal = useCallback((projectId: string, slot: number) => {
    setActiveLogProject(null);
    selectTerminalSlot(projectId, slot);
  }, [selectTerminalSlot]);
  const createProject = useCallback(() => openCreate(), [openCreate]);
  const openStopAll = useCallback(() => {
    if (!dialogOpen.current) { dialogOpen.current = true; setStopAllOpen(true); }
  }, []);
  const inTerminal = !!selectedProject && selection?.view === 'terminal';
  const focusActive = inTerminal && focusMode;
  const projectOrder = useMemo(() => sidebarNavigation(projects, groups), [projects, groups]);
  const railProjects = focusActive ? [...projectOrder].sort((a, b) =>
    Number(['running', 'starting'].includes(projectStatuses[b.id])) - Number(['running', 'starting'].includes(projectStatuses[a.id]))) : [];
  if (focusActive && selectedProject?.archived) railProjects.unshift(selectedProject);
  const activeSlot = selectedProject ? activeSlots[selectedProject.id] ?? 1 : 1;
  const slots = selectedProject ? getProjectTerminalSlots(selectedProject.id, terminals) : [];

  return (
    <TooltipProvider delayDuration={150}>
      <div className="h1-window flex h-screen cursor-default select-none overflow-hidden" data-terminal-view={inTerminal || undefined}
        data-focus-mode={focusActive || undefined} style={{ '--terminal-surface': TERMINAL_THEME.background } as CSSProperties}>
        <Toaster theme={settings.colorScheme} />

        {focusActive && selectedProject ? <ProjectFocusRail projects={railProjects} statuses={projectStatuses}
          selectedId={selectedProject.id} onSelect={selectProject} /> : <ProjectSidebar
          projects={projects}
          folderDropName={folderDropName}
          groups={groups}
          visibleProjects={visibleProjects}
          searchQuery={searchQuery}
          searchInputRef={searchInput}
          onSearchChange={setSearchQuery}
          onReorder={reorderProjects}
          reordering={reordering}
          filter={filter}
          onFilterChange={setFilter}
          attentionProjects={attentionProjects}
          changedProjects={changedProjects}
          onQuickSwitch={openQuickSwitcher}
          onUpdateGroup={updateGroup}
          onReorderGroups={reorderGroups}
          onGroupAction={(action, group) => setGroupForm({ action, group })}
          onProjectFlags={setProjectFlags}
          statuses={projectStatuses}
          conflicts={conflicts}
          selection={selection}
          maxPerProject={settings.maxPerProject}
          loading={loading}
          projectsLoaded={projectsLoaded}
          onSelectProject={selectProject}
          onCreateTerminal={openTerminal}
          onRefresh={loadProjects}
          onCreate={createProject}
          managedCount={managedProjects.length}
          onStopAll={openStopAll}
          currentVersion={updater.currentVersion}
          availableVersion={updater.update?.version}
          onCheckUpdates={updater.checkNow}
          onOpenSettings={openSettings}
        />}

        <main className={'h1-main flex min-h-0 min-w-0 flex-1 flex-col' + (inTerminal ? ' terminal-chrome' : '')}>
          {!selectedProject && <div data-tauri-drag-region className="h-10 shrink-0" aria-hidden="true" />}
          {projects.length === 0 ? (
            <Empty className="h1-group flex-1">
              <FolderPlus className="size-8 text-muted-foreground" />
              <EmptyTitle>暂无配置项目</EmptyTitle>
              <EmptyDescription>
                选择本地项目目录并设置启动命令，即可在这里启停项目和打开终端。
              </EmptyDescription>
              <Button onClick={() => openCreate()}>新建项目</Button>
            </Empty>
          ) : selectedProject ? (
                <ProjectCard
                  key={selectedProject.id}
                  project={selectedProject}
                  view={selection?.view ?? 'overview'}
                  slots={slots}
                  activeSlot={activeSlot}
                  terminals={terminals}
                  maxTerminals={settings.maxPerProject}
                  focused={focusActive}
                  onToggleFocus={toggleFocusMode}
                  onNewTerminal={() => openTerminal(selectedProject.id)}
                  onTerminalClosed={slot => onTerminalClosed(selectedProject.id, slot)}
                  terminalPane={<TerminalWorkspace ref={terminalControls} project={selectedProject} activeSlot={activeSlot}
                    slots={slots} terminals={terminals} focused={focusActive} onFocusedSlot={onFocusedTerminal} />}
                  onSelectView={view => selectView({ projectId: selectedProject.id, view })}
                  onSelectTerminal={slot => selectSidebarTerminal(selectedProject.id, slot)}
                  status={projectStatuses[selectedProject.id] || 'stopped'}
                  runtime={runtimes[selectedProject.id]}
                  conflicts={selectedConflicts}
                  onStart={handleStart}
                  onStop={handleStop}
                  onRestart={handleRestart}
                  onOpenBrowser={handleOpenBrowser}
                  onSetDefaultService={setDefaultService}
                  onRevealPath={handleRevealPath}
                  onOpenTerminal={() => selectTerminalSlot(selectedProject.id, activeSlots[selectedProject.id] ?? 1)}
                  onOpenLogs={id => { flushLogs(); setActiveLogTarget(id); setActiveLogProject(selectedProject); }}
                  onClearLogs={() => handleClearLogs(selectedProject.id)}
                  onEdit={focusPort => setProjectForm({ project: selectedProject, focusPort })}
                  onDelete={() => setDeleteProject(selectedProject)}
                  onRunCommand={command => { void handleRunCommand(selectedProject.id, command); }}
                />
          ) : null}
        </main>

        <UpdateDialog updater={updater} projects={managedProjects} terminalCount={terminalCount} />
        {quickOpen && <QuickSwitcher projects={[...projectOrder, ...projects.filter(project => project.archived)]} statuses={projectStatuses}
          terminalActions={inTerminal ? {
            focused: focusActive,
            onFocus: () => { closeQuickSwitcher(); toggleFocusMode(); },
            onSplit: () => { closeQuickSwitcher(); toggleSplit(); },
          } : undefined}
          onClose={closeQuickSwitcher}
          onCreate={() => { closeQuickSwitcher(); openCreate(); }}
          onCreateGroup={() => { closeQuickSwitcher(); dialogOpen.current = true; setGroupForm({ action: 'create' }); }}
          onSelect={project => { closeQuickSwitcher(); setSearchQuery(''); setFilter('all'); selectProject(project.id); }}
          onToggle={project => { closeQuickSwitcher(); void (['running', 'starting'].includes(projectStatuses[project.id]) ? handleStop(project.id) : handleStart(project.id)); }}
          onNewTerminal={project => { closeQuickSwitcher(); setSearchQuery(''); setFilter('all'); void openTerminal(project.id); }} />}
        {groupForm && <GroupDialog group={groupForm.group} deleting={groupForm.action === 'delete'} onClose={() => setGroupForm(null)}
          onSave={async name => {
            if (groupForm.action === 'create') await addGroup(name);
            else if (groupForm.action === 'rename') await updateGroup(groupForm.group!.id, { name });
            else { await invoke('delete_group', { id: groupForm.group!.id }); await loadProjects(); }
          }} />}
        {settingsOpen && !updater.isModalOpen && <SettingsDialog settings={settings} updater={updater}
          onClose={() => setSettingsOpen(false)}
          onCheckUpdates={() => { setSettingsOpen(false); void updater.checkNow(); }} />}

        {projectForm && <ProjectFormDialog
          project={projectForm.project}
          initialInspection={projectForm.inspection}
          focusPort={projectForm.focusPort}
          projects={projects}
          groups={groups}
          onCreateGroup={addGroup}
          status={projectForm.project ? projectStatuses[projectForm.project.id] : undefined}
          onClose={() => setProjectForm(null)}
          onSaved={async (project) => {
            await loadProjects();
            setSelection({ projectId: project.id, view: 'overview' });
            setProjectForm(null);
          }}
        />}
        {deleteProject && <ProjectDeleteDialog project={deleteProject} onClose={() => setDeleteProject(null)} onDeleted={async () => {
          const id = deleteProject.id;
          clearLogs(id, true);
          if (activeLogProject?.id === id) setActiveLogProject(null);
          await loadProjects();
          setDeleteProject(null);
        }} />}

        {externalStop && <ExternalProcessDialog
          {...externalStop}
          onClose={() => setExternalStop(null)}
          onCompleted={async () => { await refreshRuntime(); setExternalStop(null); }}
        />}

        {stopAllOpen && <StopAllProjectsDialog
          projects={managedProjects}
          onClose={() => setStopAllOpen(false)}
          onStopped={async () => { await refreshRuntime(); setStopAllOpen(false); }}
        />}

        {/* Global Terminal Logs Modal Dialog */}
        {activeLogProject && (
          <TerminalLogsModal
            project={activeLogProject}
            focusEntry={activeLogTarget}
            onClose={() => setActiveLogProject(null)}
            onClear={() => handleClearLogs(activeLogProject.id as string)}
          />
        )}
      </div>
    </TooltipProvider>
  );
}

export default App;
