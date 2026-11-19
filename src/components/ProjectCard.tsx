import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, CodeXml, Ellipsis, Globe2, Play, Square } from 'lucide-react';
import { toast } from 'sonner';
import type {
  DashboardConflict, DashboardCpuPoint, DashboardHealth, DashboardRun, DashboardServiceSample, DashboardUsage,
  PortConflict, ProjectConfig, ProjectRuntimeStatus, ProjectStatus, ProjectView, Service,
} from '../types';
import { cpuChart, dashboardPortListening, dashboardServices, dashboardStatus, durationText, relativeTime, runInterval } from '../lib/dashboard';
import { useDashboard } from '../lib/useDashboard';
import { useProjectLaunch } from '../lib/useProjectLaunch';
import type { TerminalStatus } from '../lib/terminalRegistry';
import { DashboardGitCard, DashboardOutput, DashboardStandard } from './DashboardPanels';
import { OpenServiceButton, ProjectWebMenu } from './ProjectServices';
import { ProjectTabs } from './ProjectTabs';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Empty, EmptyDescription, EmptyTitle } from './ui/empty';
import './dashboard.css';

interface ProjectCardProps {
  project: ProjectConfig;
  view: ProjectView;
  slots: number[];
  activeSlot: number;
  terminals: Record<string, TerminalStatus>;
  maxTerminals: number;
  focused: boolean;
  onToggleFocus: () => void;
  onNewTerminal: () => Promise<void>;
  onTerminalClosed: (slot: number) => void;
  terminalPane: ReactNode;
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
  onOpenTerminal: () => void;
  onOpenLogs: (id?: string) => void;
  onClearLogs: () => void;
  onEdit: (conflict?: DashboardConflict) => void;
  onDelete: () => void;
  onRunCommand: (command: string) => void;
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
  const previous = last?.port === service.port ? last : undefined;
  const check = live ? (health?.port === service.port ? health : undefined) : previous?.health;
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
          <span className="min-w-0">
            <strong className="dash-truncate" title={service.name}>{service.name}</strong>
            <small className="dash-truncate">{framework ?? process ?? folder}</small>
          </span>
        </summary>
        <div className="h1-popup dash-inspect-panel">
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
        <code>{service.port == null ? '—' : 'localhost:' + service.port}</code>
        <span className="h1-status-dot" data-tone={state === 'listening' ? 'running' : state === 'occupied' ? 'warning' : undefined}
          aria-label={statusLabel} title={statusLabel} />
        {service.port != null && <OpenServiceButton port={service.port} listening={state !== 'idle'} onOpen={onOpen}
          className="h1-tag" data-service-open={service.id}>打开 <ArrowUpRight size={13} aria-hidden="true" /></OpenServiceButton>}
      </div>
    </div>
    {active && unexpected.length > 0 && <div className="dash-port-warning" role="status" data-port-injection-warning>
      <span className="h1-status-dot" data-tone="warning" aria-hidden="true" />
      <p>该项目没有读取注入端口（实际 {unexpected.map(port => ':' + port).join('、')}）
        <small>在启动命令的端口位置使用 <code>{placeholder}</code></small></p>
    </div>}
  </section>;
}

function ResourceSummary({ usage, history, external }: { usage?: DashboardUsage | null; history?: DashboardCpuPoint[]; external: boolean }) {
  return <details className="dash-resource dash-inspect" data-dashboard-resource>
    <summary className="dash-resource-values" aria-label="查看资源曲线" title="查看资源曲线">
      <span>CPU <b>{usage ? usage.cpu.toFixed(1) + '%' : '—'}</b></span>
      <span>内存 <b>{usage ? mb(usage.rssKib) + ' MB' : '—'}</b></span>
    </summary>
    <div className="h1-popup dash-inspect-panel">
      <p className="text-meta text-muted-foreground">{external ? '外部进程' : '整个进程树'} · 最近 10 分钟</p>
      <Sparkline points={history} tone="total" />
    </div>
  </details>;
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

export function ProjectCard({ project, view, slots, activeSlot, terminals, terminalPane, onSelectView, onSelectTerminal,
  focused, maxTerminals, onToggleFocus, onNewTerminal, onTerminalClosed,
  status: runtimeStatus, runtime, conflicts, onStart, onStop, onRestart, onOpenBrowser,
  onSetDefaultService, onRevealPath, onOpenTerminal, onOpenLogs, onClearLogs, onEdit, onDelete, onRunCommand }: ProjectCardProps) {
  const { snapshot, snapshotError, git, health, now, refresh } = useDashboard(project, runtime, view);
  const launch = useProjectLaunch(project);
  const status = dashboardStatus(runtimeStatus, runtime, snapshot);
  const active = status === 'running' || status === 'starting';
  const running = status === 'running';
  const services = useMemo(() => dashboardServices(project), [project.services, project.port]);
  const blocked = !active && (conflicts.length > 0 || !!snapshot?.conflicts.length);
  const [occupied, setOccupied] = useState<DashboardConflict>();
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const defaultService = services.find(service => service.openWeb) ?? services[0];
  const startedAt = runtime?.startedAt ?? snapshot?.startedAt;
  const last = snapshot?.lastRun;
  const statusText = running ? '运行中' + (startedAt == null ? '' : ' · ' + durationText(now - startedAt))
    : status === 'starting' ? '启动中' : status === 'error' ? '运行异常' : '已停止' + (last ? ' · ' + relativeTime(last.endedAt, now) : '');
  const branch = git?.branch ?? git?.head;
  const envCount = new Set([...project.env, ...(launch?.data?.steps.flatMap(step => step.portEnv) ?? [])]
    .map(variable => variable.key)).size;

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

  const seeEditor = () => {
    void invoke('dashboard_open_editor', { id: project.id }).catch(error => toast.error(String(error)));
  };
  const verifyRelease = async () => {
    if (!occupied?.owner || !occupied.identity) return;
    await invoke('dashboard_release_port', { id: project.id, port: occupied.port, pid: occupied.owner.pid, identity: occupied.identity });
    setOccupied(undefined);
    refresh();
  };
  const showWeb = (service: Service) => dashboardPortListening(service, snapshot?.services);
  const name = project.displayName || project.name;
  const config = <details className="dash-config" data-dashboard-config>
    <summary className="h1-tag" aria-label="启动配置与更多项目操作"><Ellipsis size={16} aria-hidden="true" /></summary>
    <div className="h1-popup dash-config-panel">
      <div className="dash-config-info">
        <h3 className="h1-group-title">启动配置</h3>
        <code className="dash-truncate" title={project.path}>{project.path}</code>
        <code className="dash-truncate" title={project.startCommand}>{project.startCommand}</code>
        <p>{services.length} 个服务 · {envCount} 个环境变量</p>
      </div>
      <div className="h1-menu-list" onClick={event => {
        if ((event.target as HTMLElement).closest('button')) event.currentTarget.closest('details')?.removeAttribute('open');
      }}>
        <button type="button" onClick={() => onEdit()} data-edit-project>编辑配置</button>
        <button type="button" onClick={() => onRevealPath(project.path)} data-reveal-path>在 Finder 中定位</button>
        <button type="button" onClick={() => {
          void navigator.clipboard.writeText(project.path).then(() => toast.success('项目路径已复制')).catch(error => toast.error(String(error)));
        }}>复制路径</button>
        <button type="button" onClick={() => onOpenLogs()} data-open-logs>查看启动日志</button>
        {project.commands.map(entry => <button type="button" key={entry.name} onClick={() => onRunCommand(entry.command)} data-run-command={entry.name}>运行 {entry.name}</button>)}
        <button type="button" className="dash-error" onClick={onDelete} data-delete-project>删除项目</button>
      </div>
      <DashboardStandard project={project} plan={launch?.data} planError={launch?.error} />
    </div>
  </details>;

  return <div className="h1-workspace" data-project-details={project.id} data-project-runtime={status}>
    <header className="h1-toolbar" data-tauri-drag-region>
      <div className="h1-toolbar-heading" data-tauri-drag-region>
        <h1 className="dash-truncate" title={name} data-tauri-drag-region>{name}</h1>
        <span className="dash-badge" data-status={status} title={statusText} data-tauri-drag-region>
          <span className="h1-status-dot" data-tone={status} aria-hidden="true" /><span className="dash-truncate">{focused && running ? '运行中' : statusText}</span>
        </span>
      </div>
      <ProjectTabs projectId={project.id} view={view} activeSlot={activeSlot} slots={slots} terminals={terminals}
        maxTerminals={maxTerminals} onSelectView={onSelectView} onSelectTerminal={onSelectTerminal}
        onNewTerminal={onNewTerminal} onTerminalClosed={onTerminalClosed} />
      <div className="dash-actions">
        {!focused && (services.length > 1 ? <ProjectWebMenu project={project} canOpen={showWeb}
          onOpenBrowser={onOpenBrowser} onSetDefault={onSetDefaultService} />
          : <OpenServiceButton className="h1-button" port={defaultService.port} listening={showWeb(defaultService)}
              onOpen={onOpenBrowser} data-project-action="open" title={defaultService.port == null ? undefined : 'localhost:' + defaultService.port}>
              打开网页 <ArrowUpRight size={13} aria-hidden="true" />
            </OpenServiceButton>)}
        {!focused && active && <button type="button" className="h1-button" disabled={pending || !running} onClick={() => act('restart')}
          aria-keyshortcuts="Meta+R" title="重启 ⌘R" data-project-action="restart">重启</button>}
        {view === 'terminal' && <button type="button" className="h1-button" aria-keyshortcuts="Meta+Shift+Enter"
          aria-pressed={focused} title={(focused ? '退出专注' : '专注') + ' ⌘⇧↩'} data-terminal-focus-toggle
          onClick={onToggleFocus}>{focused ? '退出专注' : '专注'}</button>}
        {active ? <>
          <button type="button" className="h1-button h1-button-primary" disabled={pending} onClick={() => act('stop')}
            aria-keyshortcuts="Meta+." title="停止 ⌘." data-project-action="stop"><Square size={10} fill="currentColor" aria-hidden="true" />停止</button>
        </> : <button type="button" className="h1-button h1-button-primary" disabled={pending || blocked}
          onClick={() => act('start')} aria-keyshortcuts="Meta+R" title={blocked ? '请先处理端口冲突' : '启动 ⌘R'} data-project-action="start"><Play size={12} fill="currentColor" aria-hidden="true" />启动</button>}
      </div>
    </header>

    {view === 'terminal' ? terminalPane : view === 'git' ? (
      <div className="h1-project-scroll" data-project-git={project.id}>
        {git ? <DashboardGitCard git={git} now={now} onEditor={seeEditor} /> :
          <Empty className="h1-group"><EmptyTitle>暂无 Git 信息</EmptyTitle>
            <EmptyDescription>此项目的分支与文件改动会显示在这里。</EmptyDescription></Empty>}
      </div>
    ) : <div className="h1-project-scroll" data-project-overview={project.id}>
      <div className="dash-main">
        <div className="h1-group dash-summary" aria-label="服务与资源">
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
          <div className="dash-facts">
            {active && <ResourceSummary usage={snapshot?.total} external={snapshot?.external ?? runtime?.external ?? false}
              history={snapshot?.history.project} />}
            {branch && <span className="dash-fact-branch">分支 <b className="dash-truncate font-mono" title={branch}>{branch}</b></span>}
            {git && <><span>改动 <b>{git.changes}</b></span><button type="button" className="h1-tag dash-right" onClick={() => onSelectView('git')}>查看改动</button></>}
            {!active && !git && <span>资源 <b>—</b></span>}
          </div>
          {!active && <LastRunSummary run={last} now={now} />}
        </div>
        <DashboardOutput project={project} services={services} active={active} last={last} now={now} config={config}
          onTerminal={onOpenTerminal} onJump={onOpenLogs} onClear={onClearLogs} />
      </div>
    </div>}
    {occupied && <ConflictDialog conflict={occupied} onClose={() => setOccupied(undefined)} onConfirm={verifyRelease} />}
  </div>;
}
