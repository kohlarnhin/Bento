import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowRight, CircleCheck, TriangleAlert } from 'lucide-react';
import { dashboardLog, logTime, recentLogCounts, relativeTime } from '../lib/dashboard';
import { usePortStandard } from '../lib/usePortStandard';
import { useProjectLogLines } from '../lib/useProjectLogs';
import type { DashboardGit, DashboardRun, ProjectConfig, ProjectLaunchPlan, Service } from '../types';
import { LogText } from './LogText';
import { SegmentedControl } from './ui/segmented-control';

export function DashboardOutput({ project, services, active, last, now, config, onTerminal, onJump, onClear }: {
  project: ProjectConfig;
  services: Service[];
  active: boolean;
  last?: DashboardRun | null;
  now: number;
  config: ReactNode;
  onTerminal: () => void;
  onJump: (id: string) => void;
  onClear: () => void;
}) {
  const logs = useProjectLogLines(project.id);
  const lines = useMemo(() => logs.map(entry => dashboardLog(entry, services)), [logs, services]);
  const [selected, setSelected] = useState('all');
  const options = [{ value: 'all', label: '合并' }, ...(project.services.length >= 2
    ? services.map(service => ({ value: 'service:' + service.id, label: service.name })) : []),
    { value: 'problems', label: '仅问题' }];
  const value = options.some(option => option.value === selected) ? selected : 'all';
  const filtered = lines.filter(line => value === 'all' || (value === 'problems'
    ? line.severity !== 'normal' : 'service:' + line.service?.id === value));
  const counts = recentLogCounts(lines, now);
  const latestError = lines.filter(line => line.severity === 'error').at(-1);
  const log = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  useLayoutEffect(() => {
    const area = log.current;
    if (area && follow.current) area.scrollTop = area.scrollHeight;
  }, [filtered.length, lines.at(-1)?.id, value, active]);
  useLayoutEffect(() => {
    const area = log.current;
    if (!area) return;
    const observer = new ResizeObserver(() => {
      if (follow.current) area.scrollTop = area.scrollHeight;
    });
    observer.observe(area);
    return () => observer.disconnect();
  }, []);
  const exit = last?.exitCode ?? (last?.signal != null ? '信号 ' + last.signal : '未知');

  return <section className="dash-output" aria-labelledby="dash-output-heading">
    <header className="dash-output-heading">
      <h2 className="h1-group-title" id="dash-output-heading">{active ? '输出' : '上次输出'}</h2>
      {!active && logs.length > 0 && <span className="dash-faint">· 停止前最后 {logs.length} 行</span>}
      <button type="button" className="h1-tag dash-right" onClick={onTerminal}>打开终端 <ArrowRight size={13} aria-hidden="true" /></button>
      {config}
    </header>
    <div className="dash-output-panel terminal-chrome">
      <div className="dash-log-toolbar">
        <SegmentedControl className="dash-log-filters" label="过滤启动输出" options={options} value={value}
          onChange={filter => { follow.current = true; setSelected(filter); }} />
        <div className="dash-log-actions" title="最近 15 分钟">
          {active ? <>
            {counts.errors > 0 && <span className="dash-error">{counts.errors} 条错误</span>}
            <span className={counts.warnings ? 'dash-log-warning' : undefined}>{counts.warnings} 条警告</span>
            {latestError && <button type="button" className="h1-button" onClick={() => onJump(latestError.id)} data-dashboard-jump>跳到错误</button>}
          </> : <button type="button" className="h1-button" disabled={!logs.length} onClick={onClear} data-dashboard-clear>清空</button>}
        </div>
      </div>
      <div className="dash-log" ref={log} tabIndex={0} aria-label={active ? '最近输出' : '上次输出'} data-dashboard-log
        data-dim={!active || undefined} onScroll={event => {
          const area = event.currentTarget;
          follow.current = area.scrollHeight - area.scrollTop - area.clientHeight < 30;
        }}>
        {filtered.map(line => <div key={line.id} className="dash-log-line" data-severity={line.severity}
          data-service-key={line.service?.key} title={line.text}>
          <time className="dash-log-time" dateTime={new Date(line.timestamp).toISOString()}>{logTime(line.timestamp)}</time>
          {line.service && <span className="dash-log-service" title={line.service.name}>{line.service.name}</span>}
          <span className="dash-log-text"><LogText text={line.text} /></span>
        </div>)}
        {!filtered.length && <p className="dash-log-empty">{lines.length ? '没有符合筛选条件的输出' : active ? '暂无启动输出' : '启动后这里显示实时输出'}</p>}
        {!active && last && value === 'all' && logs.length > 0 && <div className="dash-log-exit" data-dashboard-exit>
          — 进程已退出（{exit}）{last.releasedPorts.length ? '，端口 ' + last.releasedPorts.join(' / ') + ' 已释放' : ''} —
        </div>}
      </div>
    </div>
  </section>;
}

export function DashboardGitCard({ git, now, onEditor }: { git: DashboardGit; now: number; onEditor: () => void }) {
  const branch = git.branch ?? (git.head ? '分离 · ' + git.head : '未建立分支');
  return <section className="dash-git" aria-labelledby="dash-git-heading" data-dashboard-git>
    <header className="dash-output-heading">
      <h2 className="h1-group-title" id="dash-git-heading">Git 改动</h2>
      <span className="dash-muted">{git.changes} 个文件</span>
      <button type="button" className="h1-tag dash-right" onClick={onEditor}>在编辑器打开</button>
    </header>
    <div className="h1-group dash-files">
      {git.files.map(file => <div key={file.path} className="dash-file">
        <span className="dash-file-status" data-status={file.status}>{file.status}</span>
        <span className="dash-truncate" title={file.path}>{file.path}</span>
        <span className="dash-file-count dash-right">
          {file.added != null && file.added > 0 && <span>+{file.added}</span>}
          {file.removed != null && file.removed > 0 && <span data-removed>−{file.removed}</span>}
        </span>
      </div>)}
      {git.changes > git.files.length && <div className="dash-file dash-faint">…还有 {git.changes - git.files.length} 个文件</div>}
      <div className="dash-kv"><span>分支</span><b className="dash-truncate" title={branch}>{branch}</b>
        {git.upstream && <span className="dash-faint dash-right" title={git.upstream}>领先 {git.upstream.split('/')[0]} {git.ahead} · 落后 {git.behind}</span>}
      </div>
      {git.latest && <div className="dash-kv"><span>最近提交</span><b className="dash-truncate" title={git.latest.subject}>{git.latest.subject}</b>
        <span className="dash-faint dash-right">{relativeTime(git.latest.timestamp, now)}</span></div>}
    </div>
  </section>;
}

export function DashboardStandard({ project, plan, planError, onViewTemplate }: {
  project: ProjectConfig; plan?: ProjectLaunchPlan; planError?: string; onViewTemplate: () => void;
}) {
  const shared = plan?.mode === 'shared' && project.injectPorts && project.services.some(service => service.port != null);
  const standard = usePortStandard(project.path, project.startCommand, project.services, shared);
  const checks = standard?.data?.services ?? [];
  const missing = checks.filter(service => !service.references.length);
  return <section className="dash-standard" aria-labelledby="dash-standard-heading" data-dashboard-standard>
    <header className="dash-ch">
      <h2 className="h1-group-title" id="dash-standard-heading">启动标准</h2>
      {shared && <button type="button" className="h1-tag dash-right" onClick={onViewTemplate} data-view-standard-template>查看模板</button>}
    </header>
    <div className="dash-standard-body">
      {planError && <p className="dash-warn">无法预览启动方式 · {planError}</p>}
      {!plan && !planError && <p className="dash-standard-note">正在识别启动方式…</p>}
      {shared && (standard?.error ? <p className="dash-warn">无法检查启动标准 · {standard.error}</p> : missing.length ? <>
        <p className="dash-standard-result dash-warn"><TriangleAlert size={15} aria-hidden="true" />脚本缺少端口变量引用</p>
        <p className="dash-standard-note">脚本未读取 {missing.map(service => service.variable).join('、')}，Bento 分配的端口不会生效。</p>
      </> : checks.length ? <>
        <p className="dash-standard-result dash-positive"><CircleCheck size={15} aria-hidden="true" />符合 Bento 标准</p>
        <p className="dash-standard-note">已为 {checks.length} 个服务找到端口变量或占位符。</p>
      </> : <p className="dash-standard-note">{standard?.data ? '当前服务未配置端口' : '正在检查端口变量…'}</p>)}
      {plan?.steps.map(step => {
        const service = project.services.find(service => service.id === step.serviceId);
        const vars = step.portEnv.filter(variable => shared || !variable.key.startsWith('BENTO_PORT'));
        return <div key={step.serviceId ?? 'root'} className="space-y-1.5" data-port-launch-method>
          {!shared && <p className="dash-standard-note">{service && <b>{service.name} · </b>}{step.description}</p>}
          {step.placeholderHint && <p className="dash-standard-note">在启动命令的端口位置可使用 <code>{'{port}'}</code>。</p>}
          {!!vars.length && <div className="dash-chips" data-port-injection-summary>
            {vars.map(variable => <code key={variable.key}>{variable.key}={variable.value}</code>)}
          </div>}
        </div>;
      })}
    </div>
  </section>;
}
