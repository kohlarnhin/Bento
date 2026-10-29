import { useEffect, useState, useCallback, useRef, useMemo, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { DragDropEvent } from '@tauri-apps/api/webview';
import { FolderPlus } from 'lucide-react';
import { toast } from 'sonner';
import { TooltipProvider } from './components/ui/tooltip';
import { ProjectCard } from './components/ProjectCard';
import { PortConflictBanner } from './components/PortConflictBanner';
import { TerminalLogsModal } from './components/TerminalLogs';
import { ProjectSidebar } from './components/ProjectSidebar';
import { ProjectTerminal } from './components/ProjectTerminal';
import { ProjectFormDialog } from './components/ProjectFormDialog';
import { ProjectDeleteDialog } from './components/ProjectDeleteDialog';
import { ExternalProcessDialog } from './components/ExternalProcessDialog';
import { StopAllProjectsDialog } from './components/StopAllProjectsDialog';
import { UpdateDialog, UpdateFooter } from './components/UpdateDialog';
import { useUpdater } from './hooks/useUpdater';
import { useSettings } from './hooks/useSettings';
import { SettingsDialog } from './components/SettingsDialog';
import Button from './components/ui/button';
import { Toaster } from './components/ui/sonner';
import { Empty, EmptyTitle, EmptyDescription } from './components/ui/empty';
import { createTerminal, getProjectTerminalSlots, getTerminalSnapshot, initTerminalRegistry, subscribeTerminals, terminalKey } from './lib/terminalRegistry';
import { useProjectLogs } from './lib/useProjectLogs';
import {
  ProjectConfig,
  PortConflict,
  ProjectsResponse,
  ProjectStatus,
  ProjectRuntimeStatus,
  ProjectSelection,
  ProjectDirInspection,
  PortOwner,
} from './types';

let sidebarDropContext: { isDialogOpen: () => boolean; openCreate: (inspection: ProjectDirInspection) => void } | undefined;

export async function handleSidebarDragDrop(event: DragDropEvent) {
  const sidebar = document.querySelector<HTMLElement>('aside.app-sidebar');
  if (!sidebar) return;
  delete sidebar.dataset.folderDrag;
  const context = sidebarDropContext;
  if (!context || context.isDialogOpen() || event.type === 'leave') return;
  const rect = sidebar.getBoundingClientRect();
  const x = event.position.x / window.devicePixelRatio;
  const y = event.position.y / window.devicePixelRatio;
  if (x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom) return;
  if (event.type === 'enter' || event.type === 'over') sidebar.dataset.folderDrag = 'true';
  else if (event.type === 'drop' && event.paths[0]) {
    try {
      const inspection = await invoke<ProjectDirInspection>('inspect_project_dir', { path: event.paths[0], excludeId: null });
      if (context.isDialogOpen()) return;
      if (inspection.isDir) context.openCreate(inspection);
      else toast.error('请拖入项目文件夹');
    } catch (error) {
      toast.error(`检查项目目录失败：${error}`);
    }
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
  const updater = useUpdater();
  const settings = useSettings();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const terminalCount = Object.values(terminals).filter(terminal => terminal.alive).length;
  const [projects, setProjects] = useState<ProjectConfig[]>([]);
  const [conflicts, setConflicts] = useState<PortConflict[]>([]);
  const [runtimes, setRuntimes] = useState<Record<string, ProjectRuntimeStatus>>({});
  const projectStatuses: Record<string, ProjectStatus> = Object.fromEntries(Object.entries(runtimes).map(([id, runtime]) => [id, runtime.status]));
  const applyRuntime = useCallback((updates: ProjectRuntimeStatus[]) => {
    setRuntimes(previous => {
      let changed = false;
      const next = { ...previous };
      for (const runtime of updates) {
        const current = previous[runtime.projectId];
        if (current && runtime.revision < current.revision) continue;
        if (current && (Object.keys(runtime) as (keyof ProjectRuntimeStatus)[]).every(key => current[key] === runtime[key])) continue;
        next[runtime.projectId] = runtime;
        changed = true;
      }
      return changed ? next : previous;
    });
  }, []);
  const refreshRuntime = useCallback(async () => {
    applyRuntime(await invoke<ProjectRuntimeStatus[]>('get_projects_runtime_status'));
  }, [applyRuntime]);
  const { logs, enqueue: enqueueLogs, flush: flushLogs, clear: clearLogs } = useProjectLogs();
  const [activeLogProject, setActiveLogProject] = useState<ProjectConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [selection, setSelection] = useState<ProjectSelection | null>(null);
  const [activeSlots, setActiveSlots] = useState<Record<string, number>>({});
  const selectTerminalSlot = useCallback((projectId: string, slot: number) => {
    setActiveSlots(previous => ({ ...previous, [projectId]: slot }));
    setSelection({ projectId, view: 'terminal' });
  }, []);
  useEffect(() => {
    setActiveSlots(previous => {
      let next = previous;
      for (const [projectId, slot] of Object.entries(previous)) {
        if (slot === 1 || terminals[terminalKey(projectId, slot)]) continue;
        const slots = getProjectTerminalSlots(projectId, terminals);
        const replacement = slots.filter(candidate => candidate < slot).at(-1) ?? slots.find(candidate => candidate > slot) ?? 1;
        if (next === previous) next = { ...previous };
        next[projectId] = replacement;
      }
      return next;
    });
  }, [terminals]);
  const watchedProjectId = selection?.view === 'terminal' ? selection.projectId : null;
  const watchedSlot = watchedProjectId ? activeSlots[watchedProjectId] ?? 1 : null;
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
  const visibleProjects = useMemo(() => projects.filter(project => !searchText ||
    [project.displayName, project.name, project.path, project.port?.toString()].some(value => value?.toLowerCase().includes(searchText))), [projects, searchText]);
  const navigation = useRef({ projects: visibleProjects, view: selection?.view || 'overview', selection, activeSlots });
  navigation.current = { projects: visibleProjects, view: selection?.view || 'overview', selection, activeSlots };
  const [reordering, setReordering] = useState(false);
  const reorderPending = useRef(false);
  const [projectForm, setProjectForm] = useState<{ project?: ProjectConfig; inspection?: ProjectDirInspection } | null>(null);
  const [deleteProject, setDeleteProject] = useState<ProjectConfig | null>(null);
  const [externalStop, setExternalStop] = useState<{ project: ProjectConfig; owners: PortOwner[]; restart: boolean } | null>(null);
  const [stopAllOpen, setStopAllOpen] = useState(false);
  const managedProjects = projects.filter(project => runtimes[project.id]?.isManaged && ['starting', 'running'].includes(runtimes[project.id].status));
  const externalRequest = useRef(false);
  const dialogOpen = useRef(false);
  dialogOpen.current = !!(projectForm || deleteProject || activeLogProject || externalStop || stopAllOpen || settingsOpen || externalRequest.current || updater.isModalOpen);
  const openSettings = useCallback(() => {
    if (dialogOpen.current) return;
    dialogOpen.current = true;
    setSettingsOpen(true);
  }, []);
  const openCreate = useCallback((inspection?: ProjectDirInspection) => {
    if (dialogOpen.current) return;
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
    const onShortcut = (event: KeyboardEvent) => {
      if (!event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.keyCode === 229) return;
      const key = event.key.toLowerCase();
      const cycle = event.shiftKey && ['BracketLeft', 'BracketRight'].includes(event.code) && navigation.current.view === 'terminal';
      if (!cycle && (event.shiftKey || (key !== 't' && key !== 'n' && key !== 'f' && key !== ',' && !/^[1-9]$/.test(key)))) return;
      event.preventDefault();
      event.stopPropagation();
      // xterm also handles keyup; swallowing the complete shortcut keeps focus
      // in search and prevents a later event from reaching the old terminal.
      if (event.type !== 'keydown') return;
      if (dialogOpen.current) return;
      if (cycle) {
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
      else if (key === 'n') openCreate();
      else if (key === 'f') { searchInput.current?.focus(); searchInput.current?.select(); }
      else {
        const project = navigation.current.projects[Number(key) - 1];
        if (project) setSelection({ projectId: project.id, view: navigation.current.view });
      }
    };
    for (const type of ['keydown', 'keypress', 'keyup'] as const) window.addEventListener(type, onShortcut, true);
    sidebarDropContext = { isDialogOpen: () => dialogOpen.current, openCreate };
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => { void handleSidebarDragDrop(payload); });
    return () => {
      for (const type of ['keydown', 'keypress', 'keyup'] as const) window.removeEventListener(type, onShortcut, true);
      sidebarDropContext = undefined;
      void unlisten.then((stop) => stop());
    };
  }, [openCreate, openSettings, openTerminal, selectTerminalSlot]);

  useEffect(() => {
    const unlisten = listen('bento-open-settings', openSettings);
    return () => { void unlisten.then(stop => stop()); };
  }, [openSettings]);

  useEffect(() => {
    if (updater.isModalOpen) setSettingsOpen(false);
  }, [updater.isModalOpen]);

  useEffect(() => {
    const unlisten = listen<{ projectId: string }>('apphub-select-project', ({ payload }) => {
      if (!navigation.current.projects.some(project => project.id === payload.projectId)) setSearchQuery('');
      setSelection({ projectId: payload.projectId, view: 'overview' });
    });
    return () => { void unlisten.then(stop => stop()); };
  }, []);

  const showToast = useCallback((msg: string) => {
    toast.error(msg);
  }, []);

  useEffect(() => {
    void initTerminalRegistry(showToast).catch(() => {});
  }, [showToast]);

  // Fetch projects and current status
  const loadProjects = useCallback(async () => {
    try {
      setLoading(true);
      const res = await invoke<ProjectsResponse>('get_projects');
      setProjects(res.projects);
      setConflicts(res.conflicts);
      setSelection((previous) => previous && res.projects.some((project) => project.id === previous.projectId)
        ? previous
        : res.projects[0] ? { projectId: res.projects[0].id, view: 'overview' } : null);

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

  const reorderProjects = useCallback(async (ids: string[]) => {
    if (reorderPending.current) return;
    reorderPending.current = true;
    setReordering(true);
    const previous = projects;
    const byId = new Map(projects.map(project => [project.id, project]));
    setProjects(ids.map(id => byId.get(id)!));
    try {
      await invoke('reorder_projects', { ids });
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

    const unlistenStatusPromise = listen<StatusPayload>('project-status', (event) => {
      applyRuntime([event.payload]);
    });

    return () => {
      clearInterval(interval);
      document.removeEventListener('visibilitychange', onVisibilityChange);
      unlistenLogPromise.then((unlisten) => unlisten());
      unlistenBatchPromise.then((unlisten) => unlisten());
      unlistenStatusPromise.then((unlisten) => unlisten());
    };
  }, [loadProjects, applyRuntime, refreshRuntime, enqueueLogs]);

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
      console.error('打开浏览器失败:', err);
      showToast(`打开浏览器失败: ${err}`);
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
  const selectedConflict = conflicts.find((conflict) => selectedProject && conflict.projectIds.includes(selectedProject.id));
  const selectView = (next: ProjectSelection) => {
    setActiveLogProject(null);
    setSelection(next);
  };

  return (
    <TooltipProvider delayDuration={150}>
      <div className="flex h-screen overflow-hidden bg-background font-sans text-foreground cursor-default select-none">
        <Toaster />

        <ProjectSidebar
          projects={projects}
          visibleProjects={visibleProjects}
          searchQuery={searchQuery}
          searchInputRef={searchInput}
          onSearchChange={setSearchQuery}
          onReorder={reorderProjects}
          reordering={reordering}
          statuses={projectStatuses}
          conflicts={conflicts}
          selection={selection}
          activeSlots={activeSlots}
          loading={loading}
          onSelect={selectView}
          onRefresh={loadProjects}
          onCreate={() => openCreate()}
          managedCount={managedProjects.length}
          updateFooter={<UpdateFooter updater={updater} onOpenSettings={openSettings} />}
          onStopAll={() => { if (!dialogOpen.current) { dialogOpen.current = true; setStopAllOpen(true); } }}
        />

        <main className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
          {selection?.view !== 'terminal' && <div data-tauri-drag-region className="h-8 shrink-0" aria-hidden="true" />}
          {projects.length === 0 ? (
            <Empty className="m-6 flex-1">
              <FolderPlus className="size-8 text-muted-foreground" />
              <EmptyTitle>暂无配置项目</EmptyTitle>
              <EmptyDescription>
                选择本地项目目录并设置启动命令，即可在这里启停项目和打开终端。
              </EmptyDescription>
              <Button onClick={() => openCreate()}>新建项目</Button>
            </Empty>
          ) : selectedProject && selection?.view === 'terminal' ? (
            <ProjectTerminal key={selectedProject.id} project={selectedProject} activeSlot={activeSlots[selectedProject.id] ?? 1}
              maxPerProject={settings.maxPerProject} onSelectSlot={slot => selectTerminalSlot(selectedProject.id, slot)} onCreate={() => openTerminal(selectedProject.id)} />
          ) : selectedProject ? (
            <div className="flex-1 overflow-y-auto px-7 pb-8 pt-3 lg:px-12 lg:pt-7" data-project-overview={selectedProject.id}>
              <div className="mx-auto flex max-w-4xl flex-col gap-5">
                {conflicts.length > 0 && !selectedConflict && <PortConflictBanner conflicts={conflicts} />}
                <ProjectCard
                  key={selectedProject.id}
                  project={selectedProject}
                  status={projectStatuses[selectedProject.id] || 'stopped'}
                  runtime={runtimes[selectedProject.id]}
                  logs={logs[selectedProject.id] || []}
                  conflict={selectedConflict}
                  onStart={handleStart}
                  onStop={handleStop}
                  onRestart={handleRestart}
                  onOpenBrowser={handleOpenBrowser}
                  onRevealPath={handleRevealPath}
                  onOpenLogs={() => { flushLogs(); setActiveLogProject(selectedProject); }}
                  onEdit={() => setProjectForm({ project: selectedProject })}
                  onDelete={() => setDeleteProject(selectedProject)}
                  onRunCommand={command => { void handleRunCommand(selectedProject.id, command); }}
                />
              </div>
            </div>
          ) : null}
        </main>

        <UpdateDialog updater={updater} projects={managedProjects} terminalCount={terminalCount} />
        {settingsOpen && !updater.isModalOpen && <SettingsDialog settings={settings} updater={updater}
          onClose={() => setSettingsOpen(false)}
          onCheckUpdates={() => { setSettingsOpen(false); void updater.checkNow(); }} />}

        {projectForm && <ProjectFormDialog
          project={projectForm.project}
          initialInspection={projectForm.inspection}
          projects={projects}
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
            logs={logs[activeLogProject.id as string] || []}
            onClose={() => setActiveLogProject(null)}
            onClear={() => handleClearLogs(activeLogProject.id as string)}
          />
        )}
      </div>
    </TooltipProvider>
  );
}

export default App;
