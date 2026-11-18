import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, Ellipsis, Play } from 'lucide-react';
import { toast } from 'sonner';
import type {
  DashboardConflict, DashboardCpuPoint, DashboardHealth, DashboardRun, DashboardServiceSample, DashboardUsage,
  PortConflict, ProjectConfig, ProjectRuntimeStatus, ProjectStatus, Service,
} from '../types';
import { cpuChart, dashboardServices, dashboardStatus, durationText, relativeTime, runInterval } from '../lib/dashboard';
import { injectedPortVariables } from '../lib/projectServices';
import { useDashboard } from '../lib/useDashboard';
import { DashboardGitCard, DashboardOutput, DashboardStandard } from './DashboardPanels';
import { ProjectAvatar } from './ProjectAvatar';
import { ProjectWebMenu } from './ProjectServices';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import './dashboard.css';

interface ProjectCardProps {
  project: ProjectConfig;
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

function ServiceTile({ service, sample, health, last, history, active, onOpen }: {
  service: Service;
  sample?: DashboardServiceSample;
  health?: DashboardHealth;
  history?: DashboardCpuPoint[];
  last?: DashboardRun['services'][string];
  active: boolean;
  onOpen: (port: number) => void;
}) {
  const serviceSample = sample?.port === service.port ? sample : undefined;
  const state = serviceSample?.state ?? 'idle';
  const live = state === 'listening' && active;
  const previous = last?.port === service.port ? last : undefined;
  const check = live ? (health?.port === service.port ? health : undefined) : previous?.health;
  const usage = live ? serviceSample?.usage : undefined;
  return <section className="dash-tile" data-dashboard-service={service.id} data-service={service.id} data-listener={state} data-idle={!live || undefined}>
    <div className="dash-tile-heading">
      <h2 className="dash-truncate" title={service.name}>{service.name}</h2>
      <code className="dash-key" data-key={service.key}>{service.key}</code>
      <span className="dash-listener dash-right" data-state={state}>{listenerLabels[state]}</span>
    </div>
    <div className="dash-port">
      <span>{service.port == null ? '—' : ':' + service.port}</span>
      {check && <small className={check.code == null || check.code < 200 || check.code >= 300 ? 'dash-error' : undefined}
        data-health={check.code ?? 'no-response'}>{healthText(check, live)}</small>}
      {live && !check && <small>检查中…</small>}
      {live && service.openWeb && service.port != null && <button type="button" className="dash-link dash-right"
        onClick={() => onOpen(service.port!)}>打开 <ArrowUpRight size={12} aria-hidden="true" /></button>}
    </div>
    <div className="dash-service-meta">
      {usage ? <>
        <span>CPU <b>{usage.cpu.toFixed(1)}%</b></span><span>内存 <b>{mb(usage.rssKib)} MB</b></span>
        {usage.process && <span className="dash-truncate" title={usage.process}>{usage.process}</span>}
      </> : <>
        {previous?.process && <span className="dash-truncate" title={previous.process}>{previous.process}</span>}
        <span className="dash-truncate" title={service.dir}>{service.dir === '.' ? './' : service.dir.replace(/\/+$/u, '') + '/'}</span>
      </>}
    </div>
    {live && <Sparkline points={history} tone={service.key === 'WEB' ? 'web' : service.key === 'API' ? 'api' : 'other'} />}
  </section>;
}

function ResourceTile({ usage, history, external }: { usage?: DashboardUsage | null; history?: DashboardCpuPoint[]; external: boolean }) {
  return <section className="dash-tile dash-resource" data-dashboard-resource>
    <div className="dash-tile-heading"><h2>资源</h2><span className="dash-faint">· {external ? '外部进程' : '整个进程树'}，近 10 分钟</span></div>
    <div className="dash-resource-values">
      <div><span>CPU</span><strong>{usage?.cpu.toFixed(1) ?? '—'} <small>%</small></strong></div>
      <div><span>内存</span><strong>{usage ? mb(usage.rssKib) : '—'} <small>MB</small></strong></div>
    </div>
    <Sparkline points={history} tone="total" />
  </section>;
}

function LastRunTile({ run, now }: { run?: DashboardRun | null; now: number }) {
  return <section className="dash-tile" data-dashboard-last-run>
    <div className="dash-tile-heading"><h2>上次运行</h2>
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

export function ProjectCard({ project, status: runtimeStatus, runtime, conflicts, onStart, onStop, onRestart, onOpenBrowser,
  onSetDefaultService, onRevealPath, onOpenTerminal, onOpenLogs, onClearLogs, onEdit, onDelete, onRunCommand }: ProjectCardProps) {
  const { snapshot, snapshotError, git, health, now, refresh } = useDashboard(project, runtime);
  const status = dashboardStatus(runtimeStatus, runtime, snapshot);
  const active = status === 'running' || status === 'starting';
  const running = status === 'running';
  const services = useMemo(() => dashboardServices(project), [project.services, project.port]);
  const blocked = !active && (conflicts.length > 0 || !!snapshot?.conflicts.length);
  const [occupied, setOccupied] = useState<DashboardConflict>();
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const firstPort = project.port;
  const liveServices = new Set(snapshot?.services.filter(service => service.state === 'listening').map(service => service.id));
  const startedAt = runtime?.startedAt ?? snapshot?.startedAt;
  const last = snapshot?.lastRun;
  const statusText = running ? '运行中' + (startedAt == null ? '' : ' · ' + durationText(now - startedAt))
    : status === 'starting' ? '启动中' : '未运行' + (last ? ' · ' + relativeTime(last.endedAt, now) + '停止' : '');
  const branch = git?.branch ?? git?.head;
  const envCount = new Set([...project.env, ...injectedPortVariables(project)].map(variable => variable.key)).size;

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    const content = contentRef.current;
    if (!viewport || !content) return;
    const syncScroll = () => {
      const narrow = viewport.getBoundingClientRect().width < 820;
      viewport.toggleAttribute('data-narrow-overflow', narrow && viewport.scrollHeight > viewport.clientHeight);
    };
    const observer = new ResizeObserver(syncScroll);
    observer.observe(viewport);
    observer.observe(content);
    syncScroll();
    return () => observer.disconnect();
  }, []);

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
  }, [active, running]);

  const seeEditor = () => {
    void invoke('dashboard_open_editor', { id: project.id }).catch(error => toast.error(String(error)));
  };
  const verifyRelease = async () => {
    if (!occupied?.owner || !occupied.identity) return;
    await invoke('dashboard_release_port', { id: project.id, port: occupied.port, pid: occupied.owner.pid, identity: occupied.identity });
    setOccupied(undefined);
    refresh();
  };
  const showWeb = (serviceId: string) => liveServices.has(serviceId);
  const openDefault = running && firstPort != null && !!snapshot?.services.some(service => service.port === firstPort && service.state === 'listening');

  return <div className="dashboard" ref={viewportRef} data-project-overview={project.id}>
    <div className="dash-main" ref={contentRef} data-project-details={project.id} data-project-runtime={status}>
      <div className="dash-drag-region" data-tauri-drag-region aria-hidden="true" />
      <header className="dash-top">
        <ProjectAvatar project={project} className="dash-avatar" />
        <div className="dash-heading">
          <div className="dash-title-row">
            <h1 className="dash-truncate" title={project.displayName || project.name}>{project.displayName || project.name}</h1>
            <span className="dash-badge" data-status={status}>{statusText}</span>
          </div>
          <div className="dash-subtitle">
            <code className="dash-truncate" title={project.path}>{project.path}</code>
            {branch && <><span className="dash-sep">·</span><span className="dash-branch">分支 <b>{branch}</b>
              {git!.ahead > 0 && ' ↑' + git!.ahead}{git!.behind > 0 && ' ↓' + git!.behind}</span></>}
            <span className="dash-sep">·</span><code className="dash-truncate" title={project.startCommand}>{project.startCommand}</code>
          </div>
        </div>
        <div className="dash-actions">
          {services.length > 1 ? <ProjectWebMenu project={project} disabled={!running} canOpen={showWeb}
            onOpenBrowser={onOpenBrowser} onSetDefault={onSetDefaultService} />
            : <button type="button" className="dash-button" disabled={!openDefault} data-project-action="open"
                onClick={() => { if (firstPort != null) onOpenBrowser(firstPort); }}>打开网页{firstPort != null && ' :' + firstPort}</button>}
          {active ? <>
            <button type="button" className="dash-button" disabled={pending || !running} onClick={() => act('restart')}
              aria-keyshortcuts="Meta+R" data-project-action="restart">重启 <kbd>⌘R</kbd></button>
            <button type="button" className="dash-button dash-primary" disabled={pending} onClick={() => act('stop')}
              aria-keyshortcuts="Meta+." data-project-action="stop">停止 <kbd>⌘.</kbd></button>
          </> : <button type="button" className="dash-button dash-primary" disabled={pending || blocked}
            onClick={() => act('start')} aria-keyshortcuts="Meta+R" data-project-action="start"><Play size={12} fill="currentColor" aria-hidden="true" />启动 <kbd>⌘R</kbd></button>}
        </div>
      </header>

      {!active && (snapshot?.conflicts ?? []).map(conflict => <section key={conflict.port} className="dash-alert" role="alert" data-dashboard-conflict={conflict.port}>
        <p title={conflict.owner?.command}><strong>端口 {conflict.port} 已被占用</strong>　{processLabel(conflict)} 正在监听，启动会失败</p>
        <div className="dash-right">
          <button type="button" className="dash-button" disabled={!conflict.owner || !conflict.identity}
            onClick={() => setOccupied(conflict)} data-dashboard-release>结束该进程</button>
          <button type="button" className="dash-button" onClick={() => onEdit(conflict)} data-dashboard-change-port>换一个端口</button>
        </div>
      </section>)}
      {!active && conflicts.map(conflict => <section key={'configs-' + conflict.port} className="dash-alert" role="alert">
        <p><strong>端口 {conflict.port} 配置重复</strong>　{conflict.projectNames.join('、')}</p>
        <button type="button" className="dash-button dash-right" onClick={() => onEdit()}>编辑配置</button>
      </section>)}
      {snapshotError && <p className="dash-alert" role="status">无法读取仪表盘 · {snapshotError}</p>}

      <div className="dash-tiles">
        {services.map(service => <ServiceTile key={service.id} service={service} active={active}
          sample={snapshot?.services.find(sample => sample.id === service.id)} health={health[service.id]}
          last={last?.services[service.id]} history={snapshot?.history[service.id]} onOpen={onOpenBrowser} />)}
        {active ? <ResourceTile usage={snapshot?.total} external={snapshot?.external ?? runtime?.external ?? false} history={snapshot?.history.project} />
          : <LastRunTile run={last} now={now} />}
      </div>

      <div className="dash-columns">
        <DashboardOutput project={project} services={services} active={active} last={last} now={now}
          onTerminal={onOpenTerminal} onJump={onOpenLogs} onClear={onClearLogs} />
        <div className="dash-rail">
          {git && <DashboardGitCard git={git} now={now} onEditor={seeEditor} />}
          <DashboardStandard project={project} services={services} />
        </div>
      </div>

      <section className="dash-config" aria-label="启动配置" data-dashboard-config>
        <span>启动配置</span><code className="dash-truncate" title={project.startCommand}>{project.startCommand}</code>
        <code>{services.length} 个服务</code><code>{envCount} 个环境变量</code>
        <div className="dash-config-actions dash-right">
          <button type="button" className="dash-link" onClick={onOpenTerminal}>在终端打开</button>
          <button type="button" className="dash-link dash-config-edit" onClick={() => onEdit()} data-edit-project>编辑配置</button>
          <details className="dash-more">
            <summary className="dash-link dash-config-edit" aria-label="更多项目操作"><Ellipsis size={15} aria-hidden="true" /></summary>
            <div className="dash-more-menu" onClick={event => { if ((event.target as HTMLElement).closest('button')) event.currentTarget.parentElement!.removeAttribute('open'); }}>
              <button type="button" onClick={() => onRevealPath(project.path)} data-reveal-path>在 Finder 中定位</button>
              <button type="button" onClick={() => {
                void navigator.clipboard.writeText(project.path).then(() => toast.success('项目路径已复制')).catch(error => toast.error(String(error)));
              }}>复制路径</button>
              <button type="button" onClick={() => onOpenLogs()} data-open-logs>查看启动日志</button>
              {project.commands.map(entry => <button type="button" key={entry.name} onClick={() => onRunCommand(entry.command)} data-run-command={entry.name}>运行 {entry.name}</button>)}
              <button type="button" className="dash-error" onClick={onDelete} data-delete-project>删除项目</button>
            </div>
          </details>
        </div>
      </section>
      {occupied && <ConflictDialog conflict={occupied} onClose={() => setOccupied(undefined)} onConfirm={verifyRelease} />}
    </div>
  </div>;
}
