import type { DashboardCpuPoint, DashboardServiceSample, DashboardSnapshot, LogEntry, ProjectConfig, ProjectRuntimeStatus, ProjectStatus, Service } from '../types.ts';

export const DASHBOARD_LOG_LIMIT = 500;

export function dashboardServices(project: Pick<ProjectConfig, 'services' | 'port'>): Service[] {
  if (project.services.length) {
    return project.services.length === 1 ? [{ ...project.services[0], name: '服务', key: 'PORT' }] : project.services;
  }
  return [{ id: 'legacy:root', name: '服务', dir: '.', key: 'PORT', port: project.port ?? null, openWeb: project.port != null }];
}

export function dashboardStatus(status: ProjectStatus, runtime?: ProjectRuntimeStatus, snapshot?: DashboardSnapshot): ProjectStatus {
  if (runtime?.isManaged) return status;
  if (snapshot?.external) return 'running';
  if (snapshot && runtime?.external) return 'stopped';
  return status;
}

export function dashboardPortListening(service: Pick<Service, 'id' | 'port'>, samples?: DashboardServiceSample[]) {
  return service.port != null && !!samples?.some(sample => sample.id === service.id &&
    sample.port === service.port && sample.state !== 'idle');
}

export function cleanTerminalText(line: string) {
  return line.replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[\x20-\x3f]*[\x40-\x7e]|\x1b[\x30-\x7e]/gu, '')
    .replace(/[\x00-\x08\x0b-\x1f\x7f]/gu, '');
}

export type DashboardSeverity = 'error' | 'warning' | 'normal';
export function logSeverity(text: string): DashboardSeverity {
  const http = /\b(?:http|get|post|put|patch|delete|head|options|status)\b/iu.test(text)
    && /(?:^|\s|["'(])5\d{2}(?:$|[\s"'),:\]])/u.test(text);
  if (/\b(?:traceback|failed|[a-z0-9_]*(?:error|exception))\b/iu.test(text) || http) return 'error';
  return /\bwarn(?:ing)?\b/iu.test(text) ? 'warning' : 'normal';
}

export function dashboardLog(entry: LogEntry, services: Service[]) {
  const rawText = cleanTerminalText(entry.text);
  const prefix = /^\s*(?:\[([^\]]+)\]|([^|\s][^|]{0,60})\|)\s*/u.exec(rawText);
  const alias = (prefix?.[1] ?? prefix?.[2])?.trim().toLowerCase();
  const service = alias ? services.find(item => [item.name, item.key, item.dir.replace(/\/+$/u, ''),
    item.dir.split('/').filter(Boolean).at(-1), item.kind].some(name => name?.trim().toLowerCase() === alias)) : undefined;
  return {
    ...entry,
    text: service && prefix ? rawText.slice(prefix[0].length) : rawText,
    service,
    severity: logSeverity(rawText),
  };
}

export function recentLogCounts(entries: { timestamp: number; severity: DashboardSeverity }[], now: number) {
  let errors = 0;
  let warnings = 0;
  for (const entry of entries) {
    if (entry.timestamp < now - 15 * 60_000) continue;
    if (entry.severity === 'error') errors++;
    if (entry.severity === 'warning') warnings++;
  }
  return { errors, warnings };
}

export function durationText(elapsed: number) {
  const minutes = Math.max(0, Math.floor(elapsed / 60_000));
  if (minutes < 1) return '不到 1 分钟';
  if (minutes < 60) return minutes + ' 分钟';
  return Math.floor(minutes / 60) + ' 小时' + (minutes % 60 ? ' ' + minutes % 60 + ' 分钟' : '');
}

export function relativeTime(timestamp: number, now: number) {
  const minutes = Math.max(0, Math.floor((now - timestamp) / 60_000));
  if (!minutes) return '刚刚';
  if (minutes < 60) return minutes + ' 分钟前';
  if (minutes < 1440) return Math.floor(minutes / 60) + ' 小时前';
  return Math.floor(minutes / 1440) + ' 天前';
}

const clock = new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' });
const minuteClock = new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
export const logTime = (timestamp: number) => clock.format(timestamp);
export function runInterval(started: number, ended: number, now: number) {
  const from = new Date(started);
  const to = new Date(ended);
  const today = new Date(now).toDateString();
  const date = (value: Date) => value.toDateString() === today ? '今天' : (value.getMonth() + 1) + '月' + value.getDate() + '日';
  return date(from) + ' ' + minuteClock.format(from) + ' – '
    + (from.toDateString() === to.toDateString() ? '' : date(to) + ' ') + minuteClock.format(to);
}

export function cpuChart(points: DashboardCpuPoint[], width: number, height: number) {
  if (!points.length) return '';
  const max = Math.max(10, ...points.map(point => point.value));
  const sample = points.length === 1 ? [points[0], points[0]] : points;
  return sample.map((point, index) => {
    const x = index * width / (sample.length - 1);
    const y = height - 4 - (height - 8) * Math.max(0, point.value) / max;
    return x.toFixed(2) + ',' + y.toFixed(2);
  }).join(' ');
}
