import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import gsap from 'gsap';
import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, CodeXml, Folder, GitBranch, Globe2, Maximize2, Minimize2, Pencil, Play, Square, SquareTerminal } from 'lucide-react';
import type {
  DashboardConflict, DashboardCpuPoint, DashboardGit, DashboardHealth, DashboardRun, DashboardServiceSample, DashboardUsage,
  PortConflict, ProjectConfig, ProjectRuntimeStatus, ProjectStatus, ProjectView, Service, Snippet, SnippetChange,
} from '../types';
import { cpuChart, dashboardListeningPort, dashboardServices, dashboardStatus, durationText, relativeTime, runInterval } from '../lib/dashboard';
import { useDashboard } from '../lib/useDashboard';
import { useProjectLaunch } from '../lib/useProjectLaunch';
import { usePortStandard } from '../lib/usePortStandard';
import { useReducedMotion } from '../lib/motion';
import { dashboardEntranceLimit, readyProjectView } from '../lib/motionMath';
import type { TerminalStatus } from '../lib/terminalRegistry';
import type { OpenAgent } from '../lib/agentSessions';
import { useProjectViewMotion } from '../hooks/useProjectViewMotion';
import { DashboardGitCard, DashboardOutput } from './DashboardPanels';
import { OpenServiceButton, ProjectWebMenu } from './ProjectServices';
import { ProjectTabs } from './ProjectTabs';
import { TerminalWorkspace, type TerminalWorkspaceControls } from './TerminalWorkspace';
import { StatusDot } from './StatusDot';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Empty, EmptyDescription, EmptyTitle } from './ui/empty';
import { ActionTooltip } from './ui/tooltip';
import { MotionPresence } from './ui/motion-presence';
import './dashboard.css';

interface ProjectCardProps {
  project: ProjectConfig;
  view: ProjectView;
  slots: number[];
  activeSlot: number;
  terminals: Record<string, TerminalStatus>;
  maxTerminals: number;
  showQuickCommands: boolean;
  snippets: Snippet[];
  onRunSnippet: (command: string) => void;
  onChangeSnippet: (change: SnippetChange) => Promise<void>;
  focused: boolean;
  onToggleFocus: () => void;
  onNewTerminal: () => Promise<void>;
  onOpenAgent: OpenAgent;
  closeActiveTab: RefObject<() => void>;
  terminalControls: RefObject<TerminalWorkspaceControls>;
  onFocusedTerminal: (projectId: string, slot: number, left: number, right?: number) => void;
  onJumpToAttention: () => void;
  onSelectView: (view: 'overview' | 'git') => void;
  onSelectTerminal: (slot: number) => void;
  status: ProjectStatus;
  runtime?: ProjectRuntimeStatus;
  conflicts: PortConflict[];
  onStart: (id: string) => Promise<void>;
  onStop: (id: string) => Promise<void>;
  onRestart: (id: string) => Promise<void>;
  onOpenBrowser: (port: number) => void;
  onSetDefaultService: (serviceId: string) => Promise<void>;
  onRevealPath: (path: string) => void;
  onOpenEditor: () => void;
  onOpenTerminal: () => void;
  onOpenLogs: (id?: string) => void;
  onClearLogs: () => void;
  onEdit: (conflict?: DashboardConflict) => void;
}

const listenerLabels = { listening: '监听中', idle: '未监听', occupied: '端口被占用' };
const mb = (kib: number) => Math.round(kib / 1024);

function Sparkline({ points = [], tone }: { points?: DashboardCpuPoint[]; tone: 'web' | 'api' | 'total' | 'other' }) {
  const line = cpuChart(points, 300, 26);
  return <svg className="dash-spark" data-tone={tone} data-sample-count={points.length} viewBox="0 0 300 26" preserveAspectRatio="none" aria-hidden="true">
    {line && <>
      <polygon points={'0,26 ' + line + ' 300,26'} fill="currentColor" opacity=".1" />
      <polyline points={line} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </>}
  </svg>;
}

function healthText(health: DashboardHealth, active: boolean) {
  const endpoint = health.path === '/' ? 'HTTP' : health.path;
  if (!active) return health.code == null ? '上次无响应' : '上次 ' + endpoint + ' ' + health.code;
  return health.code == null ? '无响应' : endpoint + ' ' + health.code + ' · ' + health.elapsedMs + 'ms';
}

function ServiceRow({ service, sample, health, last, history, active, framework, onOpen }: {
  service: Service;
  sample?: DashboardServiceSample;
  health?: DashboardHealth;
  history?: DashboardCpuPoint[];
  last?: DashboardRun['services'][string];
  active: boolean;
  framework?: string;
  onOpen: (port: number) => void;
}) {
  const serviceSample = sample?.port === service.port ? sample : undefined;
  const state = serviceSample?.state ?? 'idle';
  const live = state === 'listening' && active;
  const port = serviceSample?.actualPort ?? service.port;
  const previous = last?.port === service.port ? last : undefined;
  const check = live ? (health?.port === port ? health : undefined) : previous?.health;
  const usage = live ? serviceSample?.usage : undefined;
  const unexpected = serviceSample?.unexpectedPorts ?? [];
  const placeholder = service.key === 'PORT' ? '{port}' : '{port:' + service.key + '}';
  const process = usage?.process ?? previous?.process;
  const folder = service.dir === '.' ? './' : service.dir.replace(/\/+$/u, '') + '/';
  const statusLabel = listenerLabels[state] + (check ? ' · ' + healthText(check, live) : live ? ' · 检查中…' : '');
  const Icon = service.kind === 'api' ? CodeXml : Globe2;
  return <section className="dash-service" data-dashboard-service={service.id} data-service={service.id} data-listener={state} data-idle={!live || undefined}>
    <div className="dash-service-row">
      <details className="dash-inspect">
        <summary aria-label={service.name + '服务详情'} title={statusLabel}>
          <span className="dash-service-icon"><Icon size={14} aria-hidden="true" /></span>
          <span className="dash-service-label">
            <strong className="dash-truncate" title={service.name}>{service.name}</strong>
            <small className="dash-truncate">{framework ?? process ?? folder}</small>
          </span>
        </summary>
        <div className="dash-service-details">
          <div className="dash-detail-line"><code data-key={service.key}>{service.key}</code><code>{folder}</code></div>
          <p className={check && (check.code == null || check.code < 200 || check.code >= 300) ? 'dash-error' : 'text-muted-foreground'}
            data-health={check ? check.code ?? 'no-response' : undefined}>{statusLabel}</p>
          {(process || usage) && <div className="dash-detail-line">
            {process && <span>{process}</span>}
            {usage && <><span>CPU <b>{usage.cpu.toFixed(1)}%</b></span><span>内存 <b>{mb(usage.rssKib)} MB</b></span></>}
          </div>}
          {live && <Sparkline points={history} tone={service.key === 'WEB' ? 'web' : service.key === 'API' ? 'api' : 'other'} />}
        </div>
      </details>
      <div className="dash-service-address">
        <code>{port == null ? '—' : 'localhost:' + port}</code>
        <span className="h1-status-dot" data-tone={state === 'listening' ? 'running' : state === 'occupied' ? 'warning' : undefined}
          aria-label={statusLabel} title={statusLabel} />
        {port != null && <OpenServiceButton port={port} listening={state !== 'idle'} onOpen={onOpen}
          className="h1-tag" aria-label={`打开 ${service.name}`} data-service-open={service.id}><ArrowUpRight size={14} aria-hidden="true" /></OpenServiceButton>}
      </div>
    </div>
    {active && unexpected.length > 0 && <div className="dash-port-warning" role="status" data-port-injection-warning>
      <span className="h1-status-dot" data-tone="warning" aria-hidden="true" />
      <p>该项目没有读取注入端口（实际 {unexpected.map(port => ':' + port).join('、')}）
        <small>在启动命令的端口位置使用 <code>{placeholder}</code></small></p>
    </div>}
  </section>;
}

function ResourceSummary({ active, uptime, usage, history, external }: {
  active: boolean; uptime: string; usage?: DashboardUsage | null; history?: DashboardCpuPoint[]; external: boolean;
}) {
  const values = <span className="dash-resource-values">
    <span>CPU <b>{usage ? usage.cpu.toFixed(1) + '%' : '—'}</b></span>
    <span>内存 <b>{usage ? mb(usage.rssKib) + ' MB' : '—'}</b></span>
    <span className="dash-resource-uptime">运行 <b title={uptime}>{uptime}</b></span>
  </span>;
  if (!active) return values;
  return <details className="dash-resource dash-inspect" data-dashboard-resource>
    <summary aria-label="查看资源曲线" title="查看资源曲线">
      {values}
    </summary>
    <div className="h1-popup dash-inspect-panel">
      <p className="text-meta text-muted-foreground">{external ? '外部进程' : '整个进程树'} · 最近 10 分钟</p>
      <Sparkline points={history} tone="total" />
    </div>
  </details>;
}

function ProjectQuickActions({ git, unconfigured, terminalLimit, onEditor, onTerminal, onGit, onReveal, onEdit }: {
  git: DashboardGit | null | undefined;
  unconfigured: boolean;
  terminalLimit: boolean;
  onEditor: () => void;
  onTerminal: () => void;
  onGit: () => void;
  onReveal: () => void;
  onEdit: () => void;
}) {
  const gitSummary = git ? (git.branch ?? git.head ?? '未建立分支') + ` · ${git.changes} 处改动` : '';
  return <section className="h1-group dash-card" aria-label="快捷操作" data-dashboard-quick-actions>
    <header className="dash-card-heading" data-workspace-content>
      <h2>快捷操作</h2>
      {unconfigured && <button type="button" className="dash-launch-notice" onClick={onEdit} data-dashboard-unconfigured>未配置启动</button>}
    </header>
    <div className="dash-quick-list" data-workspace-content>
      <button type="button" aria-keyshortcuts="Meta+O" onClick={onEditor} data-dashboard-quick="editor">
        <CodeXml size={16} aria-hidden="true" />在编辑器打开<kbd aria-hidden="true">⌘O</kbd>
      </button>
      <button type="button" aria-keyshortcuts="Meta+T" disabled={terminalLimit} onClick={onTerminal} data-dashboard-quick="terminal">
        <SquareTerminal size={16} aria-hidden="true" />新终端<kbd aria-hidden="true">⌘T</kbd>
      </button>
      <button type="button" onClick={onGit} data-dashboard-quick="git">
        <GitBranch size={16} aria-hidden="true" />查看改动
        {gitSummary && <small className="dash-truncate" title={gitSummary}>{gitSummary}</small>}
      </button>
      <button type="button" aria-keyshortcuts="Alt+Meta+R" onClick={onReveal} data-reveal-path>
        <Folder size={16} aria-hidden="true" />在 Finder 中显示<kbd aria-hidden="true">⌥⌘R</kbd>
      </button>
      <button type="button" aria-keyshortcuts="Meta+," onClick={onEdit} data-edit-project>
        <Pencil size={16} aria-hidden="true" />编辑配置<kbd aria-hidden="true">⌘,</kbd>
      </button>
    </div>
  </section>;
}

function LastRunSummary({ run, now }: { run?: DashboardRun | null; now: number }) {
  return <section className="dash-last-run" data-dashboard-last-run>
    <div className="dash-last-heading"><h2>上次运行</h2>
      {run && <span className="dash-faint dash-truncate">· {runInterval(run.startedAt, run.endedAt, now)}</span>}
    </div>
    {run ? <dl className="dash-last">
      <div><dt>时长</dt><dd>{durationText(run.endedAt - run.startedAt)}</dd></div>
      <div><dt>退出</dt><dd className={run.exitCode === 0 ? 'dash-positive' : run.exitCode != null || run.signal != null ? 'dash-error' : undefined}>
        {run.exitCode === 0 ? '正常 · 0' : run.exitCode != null ? '异常 · ' + run.exitCode : run.signal != null ? '信号 · ' + run.signal : '未知'}
      </dd></div>
      <div><dt>峰值内存</dt><dd>{run.peakKib == null ? '—' : mb(run.peakKib) + ' MB'}</dd></div>
      <div><dt>问题</dt><dd className={run.errors ? 'dash-error' : undefined}>{run.external ? '—' : run.errors + ' 错误'}</dd></div>
    </dl> : <p className="dash-no-history">暂无运行记录</p>}
  </section>;
}

function processLabel(conflict: DashboardConflict) {
  const owner = conflict.owner;
  if (!owner) return '有其他进程';
  const command = owner.command.trim().split(/\s+/u)[0].split('/').at(-1) || '进程';
  return command + '（PID ' + owner.pid + (owner.cwd ? '，' + owner.cwd : '') + '）';
}

function ConflictDialog({ conflict, onClose, onConfirm }: {
  conflict: DashboardConflict;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose(); }}>
    <DialogContent hideClose={busy} data-dashboard-conflict-dialog>
      <DialogHeader>
        <DialogTitle>结束占用端口的进程</DialogTitle>
        <DialogDescription>将向 {processLabel(conflict)} 发送 SIGTERM，以释放端口 {conflict.port}。</DialogDescription>
      </DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
        <Button variant="destructive" disabled={busy} data-confirm-stop-listener onClick={() => {
          setBusy(true);
          setError('');
          void onConfirm().catch(reason => setError(String(reason))).finally(() => setBusy(false));
        }}>{busy ? '等待进程退出…' : '发送 SIGTERM'}</Button>
      </div>
    </DialogContent>
  </Dialog>;
}

export function ProjectCard({ project, view, slots, activeSlot, terminals, terminalControls, onFocusedTerminal, onJumpToAttention, onSelectView, onSelectTerminal,
  focused, maxTerminals, showQuickCommands, snippets, onRunSnippet, onChangeSnippet, onToggleFocus, onNewTerminal, onOpenAgent, closeActiveTab,
  status: runtimeStatus, runtime, conflicts, onStart, onStop, onRestart, onOpenBrowser,
  onSetDefaultService, onRevealPath, onOpenEditor, onOpenTerminal, onOpenLogs, onClearLogs, onEdit }: ProjectCardProps) {
  const { snapshot, snapshotError, git, health, now, refresh } = useDashboard(project, runtime, view);
  const launch = useProjectLaunch(project);
  const auditLaunch = launch?.data?.mode === 'shared' && project.injectPorts && project.services.some(service => service.port != null);
  const standard = usePortStandard(project.path, project.startCommand, project.services, auditLaunch);
  const unconfigured = (!project.startCommand.trim() && !project.services.some(service => service.startCommand?.trim())) ||
    !!launch?.error || !!standard?.data?.services.some(service => !service.references.length);
  const status = dashboardStatus(runtimeStatus, runtime, snapshot);
  const active = !!runtime?.isManaged || status === 'running' || status === 'starting';
  const running = status === 'running';
  const services = useMemo(() => dashboardServices(project), [project.services, project.port]);
  const blocked = !active && (conflicts.length > 0 || !!snapshot?.conflicts.length);
  const [occupied, setOccupied] = useState<DashboardConflict>();
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const defaultService = services.find(service => service.openWeb) ?? services[0];
  const startedAt = runtime?.startedAt ?? snapshot?.startedAt;
  const uptime = active && startedAt != null ? durationText(now - startedAt) : '—';
  const last = snapshot?.lastRun;
  const statusText = running ? '运行中' + (startedAt == null ? '' : ' · ' + durationText(now - startedAt))
    : status === 'starting' ? '启动中' : status === 'error' ? '运行异常' : '已停止' + (last ? ' · ' + relativeTime(last.endedAt, now) : '');
  const listeningCount = snapshot?.services.filter(service => service.state === 'listening').length ?? 0;
  const reduced = useReducedMotion();
  const workspace = useRef<HTMLDivElement>(null);
  const overviewContent = useRef<HTMLDivElement>(null);
  const [visitedTerminal, setVisitedTerminal] = useState(false);
  const [readySlot, setReadySlot] = useState<number>();
  const [lastShownView, setLastShownView] = useState<ProjectView>();
  const terminalView = view === 'terminal';
  const terminalLimit = slots.length >= maxTerminals;
  const mountTerminal = slots.includes(activeSlot) && (terminalView || visitedTerminal);
  const terminalReady = readySlot === activeSlot;
  const shown = readyProjectView(view, lastShownView, terminalReady, git !== undefined);
  const terminalInteractive = terminalView && shown === 'terminal';
  useLayoutEffect(() => {
    if (terminalView) setVisitedTerminal(true);
    if (!mountTerminal) setReadySlot(undefined);
  }, [terminalView, mountTerminal]);
  useLayoutEffect(() => { setLastShownView(shown); }, [shown]);
  useProjectViewMotion(workspace, shown, reduced);

  const launchLabel = useRef<HTMLSpanElement>(null);
  const lastLaunchState = useRef(active);
  useLayoutEffect(() => {
    const area = overviewContent.current;
    if (reduced || !area) return;
    const scope = gsap.context(() => {
      const blocks = gsap.utils.toArray('.dash-service, .dash-facts').slice(0, dashboardEntranceLimit);
      gsap.fromTo(blocks, { opacity: 0, y: 8 }, {
        opacity: 1, y: 0, stagger: .03, duration: .2, ease: 'power3.out', clearProps: 'opacity,transform',
      });
    }, area);
    return () => scope.revert();
  }, [project.id, reduced]);
  useLayoutEffect(() => {
    const previous = lastLaunchState.current;
    lastLaunchState.current = active;
    const label = launchLabel.current;
    if (reduced || previous === active || !label) return;
    const layers = [...label.children];
    const scope = gsap.context(() => {
      gsap.fromTo(layers, { opacity: (index: number) => Number(Boolean(index) === previous) },
        { opacity: (index: number) => Number(Boolean(index) === active), duration: .16, ease: 'power3.out', clearProps: 'opacity' });
    }, label);
    return () => scope.revert();
  }, [active, reduced]);

  useEffect(() => {
    if (!active && startedAt != null) refresh();
  }, [active, startedAt, refresh]);

  const act = (kind: 'start' | 'restart' | 'stop') => {
    if (busy.current || (kind === 'start' && blocked)) return;
    busy.current = true;
    setPending(true);
    const action = kind === 'start' ? onStart : kind === 'restart' ? onRestart : onStop;
    void action(project.id).finally(() => {
      setPending(false);
      busy.current = false;
      refresh();
    });
  };
  const invokeShortcut = useRef(act);
  invokeShortcut.current = act;
  useEffect(() => {
    if (view === 'terminal') return;
    const onKey = (event: KeyboardEvent) => {
      if (!event.metaKey || event.shiftKey || event.ctrlKey || event.altKey || event.repeat || event.isComposing
        || event.keyCode === 229 || document.querySelector('[role="dialog"][data-state="open"]')
        || (event.target instanceof HTMLElement && (event.target.matches('input, textarea') || event.target.isContentEditable))) return;
      const action = event.key.toLowerCase() === 'r' ? running ? 'restart' : active ? undefined : 'start'
        : event.key === '.' && active ? 'stop' : undefined;
      if (!action) return;
      event.preventDefault();
      event.stopPropagation();
      invokeShortcut.current(action);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [active, running, view]);

  const verifyRelease = async () => {
    if (!occupied?.owner || !occupied.identity) return;
    await invoke('dashboard_release_port', { id: project.id, port: occupied.port, pid: occupied.owner.pid, identity: occupied.identity });
    setOccupied(undefined);
    refresh();
  };
  const listeningPort = (service: Service) => dashboardListeningPort(service, snapshot?.services)
    ?? (project.services.length === 0 ? runtime?.actualPort : runtime?.services.find(item => item.serviceId === service.id)?.actualPort)
    ?? null;
  const openPort = runtime?.actualPort ?? listeningPort(defaultService);
  const name = project.displayName || project.name;
  const focusLabel = focused ? '退出专注' : '专注';
  const FocusIcon = focused ? Minimize2 : Maximize2;

  return <div ref={workspace} className="h1-workspace" data-shown-view={shown} data-project-details={project.id} data-project-runtime={status}>
    <header className="h1-toolbar" data-tauri-drag-region="deep">
      <div className="h1-toolbar-heading">
        <h1 className="dash-truncate" title={name}>{name}</h1>
        <span className="dash-badge" data-status={status} title={statusText}>
          <StatusDot tone={status} aria-hidden="true" /><span className="dash-truncate">{statusText}</span>
        </span>
      </div>
      <ProjectTabs projectId={project.id} view={view} activeSlot={activeSlot} slots={slots} terminals={terminals}
        maxTerminals={maxTerminals} onSelectView={onSelectView} onSelectTerminal={onSelectTerminal}
        onNewTerminal={onNewTerminal} onOpenAgent={onOpenAgent} closeActiveTab={closeActiveTab} />
      <div className="dash-actions">
        {services.length > 1 ? <ProjectWebMenu project={project} listeningPort={listeningPort} actualPort={openPort} terminal={terminalView}
          onOpenBrowser={onOpenBrowser} onSetDefault={onSetDefaultService} />
          : <OpenServiceButton className="h1-button" port={openPort ?? defaultService.port} listening={openPort != null}
              onOpen={onOpenBrowser} data-project-action="open" title={openPort == null ? undefined : 'localhost:' + openPort}>
              打开网页 <ArrowUpRight size={13} aria-hidden="true" />
            </OpenServiceButton>}
        {active && <button type="button" className="h1-button" disabled={pending || status === 'starting'} onClick={() => act('restart')}
          aria-keyshortcuts="Meta+R" title="重启 ⌘R" data-project-action="restart">重启</button>}
        <ActionTooltip label={focusLabel + ' ⌘⇧↩'}>
          <button type="button" className="h1-button terminal-focus-toggle" aria-label={focusLabel} aria-keyshortcuts="Meta+Shift+Enter"
            data-view-control aria-hidden={!terminalView || undefined} disabled={!terminalView}
            aria-pressed={focused} data-terminal-focus-toggle onClick={onToggleFocus}>
            <FocusIcon size={14} aria-hidden="true" />
          </button>
        </ActionTooltip>
        <button type="button" className="h1-button h1-button-primary" disabled={pending || (!active && blocked)}
          onClick={() => act(active ? 'stop' : 'start')} aria-label={active ? '停止' : '启动'}
          aria-keyshortcuts={active ? 'Meta+.' : 'Meta+R'} title={active ? '停止 ⌘.' : blocked ? '请先处理端口冲突' : '启动 ⌘R'}
          data-project-action={active ? 'stop' : 'start'}>
          <span ref={launchLabel} className="launch-label" data-running={active || undefined} aria-hidden="true">
            <span><Play size={12} fill="currentColor" />启动</span><span><Square size={10} fill="currentColor" />停止</span>
          </span>
        </button>
      </div>
    </header>

    <div className="workspace-views">
      <div className="workspace-view" data-workspace-view="overview" {...{ inert: shown !== 'overview' || view !== 'overview' ? '' : undefined }}>
        <div ref={overviewContent} className="dash-main" data-project-overview={project.id}>
          <div className="dash-top">
            <section className="h1-group dash-card dash-summary" aria-label="服务与资源">
              <header className="dash-card-heading" data-workspace-content><h2>服务</h2>
                <span>{active ? `${listeningCount} 个运行中` : `${services.length} 个服务`}</span>
              </header>
              <div className="dash-service-list" role="region" aria-label="服务列表" tabIndex={0} data-workspace-content>
                {services.map(service => {
                  const step = launch?.data?.steps.find(item => item.serviceId === service.id || (services.length === 1 && item.serviceId == null));
                  const framework = step?.description.match(/^已识别 ([^，]+)，/u)?.[1];
                  return <ServiceRow key={service.id} service={service} active={active} framework={framework}
                    sample={snapshot?.services.find(sample => sample.id === service.id)} health={health[service.id]}
                    last={last?.services[service.id]} history={snapshot?.history[service.id]} onOpen={onOpenBrowser} />;
                })}
                {!active && (snapshot?.conflicts ?? []).map(conflict => <section key={conflict.port} className="dash-alert" role="alert" data-dashboard-conflict={conflict.port}>
                  <span className="h1-status-dot" data-tone="warning" aria-hidden="true" />
                  <p title={conflict.owner?.command}><strong>端口 {conflict.port} 已被占用</strong> <small>{processLabel(conflict)} 正在监听，启动会失败</small></p>
                  <div className="dash-alert-actions">
                    <button type="button" className="h1-tag" disabled={!conflict.owner || !conflict.identity}
                      onClick={() => setOccupied(conflict)} data-dashboard-release>结束该进程</button>
                    <button type="button" className="h1-tag" onClick={() => onEdit(conflict)} data-dashboard-change-port>换一个端口</button>
                  </div>
                </section>)}
                {!active && conflicts.map(conflict => <section key={'configs-' + conflict.port} className="dash-alert" role="alert">
                  <span className="h1-status-dot" data-tone="warning" aria-hidden="true" />
                  <p><strong>端口 {conflict.port} 配置重复</strong> <small>{conflict.projectNames.join('、')}</small></p>
                  <button type="button" className="h1-tag dash-right" onClick={() => onEdit()}>编辑配置</button>
                </section>)}
                {snapshotError && <p className="dash-alert" role="status">无法读取仪表盘 · {snapshotError}</p>}
                {!active && <LastRunSummary run={last} now={now} />}
              </div>
              <div className="dash-facts"><div data-workspace-content>
                <ResourceSummary active={active} uptime={uptime} usage={snapshot?.total}
                  external={snapshot?.external ?? runtime?.external ?? false} history={snapshot?.history.project} />
              </div></div>
            </section>
            <ProjectQuickActions git={git} unconfigured={unconfigured} terminalLimit={terminalLimit}
              onEditor={onOpenEditor} onTerminal={() => { void onNewTerminal(); }} onGit={() => onSelectView('git')}
              onReveal={() => onRevealPath(project.path)} onEdit={() => onEdit()} />
          </div>
          <DashboardOutput project={project} services={services} active={active} last={last} now={now}
            onTerminal={onOpenTerminal} onJump={onOpenLogs} onClear={onClearLogs} />
        </div>
      </div>
      <div className="workspace-view" data-workspace-view="git" {...{ inert: shown !== 'git' || view !== 'git' ? '' : undefined }}>
        <div className="h1-project-scroll" data-project-git={project.id} data-workspace-content>
          {git ? <DashboardGitCard git={git} now={now} onEditor={onOpenEditor} /> :
            <Empty className="h1-group"><EmptyTitle>暂无 Git 信息</EmptyTitle>
              <EmptyDescription>此项目的分支与文件改动会显示在这里。</EmptyDescription></Empty>}
        </div>
      </div>
      <div className="workspace-view terminal-chrome" data-workspace-view="terminal" {...{ inert: !terminalInteractive ? '' : undefined }}>
        {mountTerminal && <TerminalWorkspace ref={terminalControls} project={project} activeSlot={activeSlot} slots={slots} terminals={terminals}
          showQuickCommands={showQuickCommands} snippets={snippets} onRunSnippet={onRunSnippet} onChangeSnippet={onChangeSnippet}
          onJumpToAttention={onJumpToAttention}
          focused={focused} interactive={terminalInteractive} ready={terminalReady} onReady={setReadySlot} onFocusedSlot={onFocusedTerminal} />}
      </div>
    </div>
    <MotionPresence>{occupied && <ConflictDialog conflict={occupied} onClose={() => setOccupied(undefined)} onConfirm={verifyRelease} />}</MotionPresence>
  </div>;
}
