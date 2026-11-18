import { AlertCircle, AlertTriangle, ArrowRight, Check } from 'lucide-react';
import type { diffProjectInspection, RescanField, RescanRow } from '../lib/projectRescan';
import { missingPortReference } from '../lib/projectServices';
import type { PortStandardCheck } from '../types';
import { cn } from '../lib/utils';
import Button from './ui/button';

const labels = { startCommand: '启动命令', port: '端口', type: '类型' };
const confidenceLabels = { high: '高置信度', medium: '中置信度', low: '低置信度' };
const displayValue = (field: RescanRow['field'], value: string) => field === 'type' ? (value === 'app' ? '桌面应用' : 'Web 项目') : value.trim() || '未填写';

type ServiceDiff = Pick<ReturnType<typeof diffProjectInspection>, 'serviceChanges' | 'startScripts'>;

export function ProjectRescanCard({ rows, sourceFiles = [], serviceChanges = [], startScripts = [], applied = false, checkStandard,
  standard, standardError, onViewTemplate, selected, error, disabled, onToggle, onDismiss, onApply }: {
  rows?: RescanRow[];
  sourceFiles?: string[];
  serviceChanges?: ServiceDiff['serviceChanges'];
  startScripts?: ServiceDiff['startScripts'];
  applied?: boolean;
  checkStandard?: boolean;
  standard?: PortStandardCheck;
  standardError?: string;
  onViewTemplate?: () => void;
  selected: ReadonlySet<RescanField>;
  error?: string;
  disabled: boolean;
  onToggle: (field: RescanField) => void;
  onDismiss: () => void;
  onApply: () => void;
}) {
  if (error) return <div role="alert" className="flex items-start gap-2 rounded-lg border border-destructive/15 bg-destructive/5 px-3.5 py-3 text-xs leading-relaxed text-destructive">
    <AlertCircle aria-hidden="true" className="size-4 shrink-0" />{error}
  </div>;
  const changeCount = (rows?.filter(row => row.changed).length ?? 0) + Number(serviceChanges.length > 0);
  if (!changeCount && !applied && !checkStandard) return <div role="status" className="flex items-center gap-2 rounded-lg border border-success/15 bg-success-muted px-3.5 py-3 text-xs text-success-foreground">
    <Check aria-hidden="true" className="size-4 shrink-0" />配置已是最新
  </div>;

  const discovery = !!(serviceChanges.length || startScripts.length);
  const missing = standard?.services.filter(service => !service.references.length) ?? [];
  const summary = serviceChanges.map(({ service, added, previousPort }) => {
    const port = service.port == null ? '未检测到端口' : `${added ? '' : (previousPort || '未填写') + ' → '}${service.port}`;
    return `${service.dir}/（${[service.portCandidate?.source, port, service.portCandidate && confidenceLabels[service.portCandidate.confidence]].filter(Boolean).join(' · ')}）`;
  }).concat(startScripts.map(script => script.source));
  return <section aria-label="重新扫描结果" className="overflow-hidden rounded-xl border border-[var(--service-diff-border)] bg-[var(--service-diff-bg)] text-xs">
    <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 px-4 py-3.5">
      <h4 role="status" className="shrink-0 text-[13px] font-semibold text-[var(--service-blue-text)]">
        {!changeCount && !applied ? '启动标准检查' : discovery
          ? `扫描到 ${serviceChanges.length} 个服务和 ${startScripts.length} 个启动脚本` : `发现 ${changeCount} 处变化`}
      </h4>
      {!!sourceFiles.length && <span className="min-w-0 break-words text-[11px] text-muted-foreground">来自 {sourceFiles.join(' · ')}</span>}
      {discovery && <p className="w-full break-words leading-relaxed text-[var(--service-diff-text)]">{summary.join('、')}。{applied
        ? '已填入表单，保存后才生效。' : changeCount > 0 && '点「应用」只填入表单，保存后才生效。'}</p>}
    </div>
    {checkStandard && <div className="space-y-2 border-t border-[var(--service-diff-border)] px-4 py-3" data-rescan-port-standard aria-live="polite">
      {standardError ? <p className="text-warning-foreground">无法检查启动标准。{standardError}</p> : !standard
        ? <p className="text-muted-foreground">正在检查启动脚本读取的端口变量…</p> : missing.length > 0
          ? <ul className="space-y-1 text-warning-foreground">{missing.map(service => <li key={service.serviceId}
            className="flex items-start gap-2" data-standard-warning={service.variable}>
            <AlertTriangle aria-hidden="true" className="size-3.5 shrink-0" />{missingPortReference(service.variable)}
          </li>)}</ul>
          : <p className="flex items-center gap-2 text-success-foreground"><Check aria-hidden="true" className="size-3.5" />
            {standard.services.length ? '已找到各服务读取 Bento 标准端口变量的代码' : '当前服务没有配置监听端口'}</p>}
      {onViewTemplate && <button type="button" className="rounded text-[var(--service-blue)] hover:underline focus-visible:outline-ring"
        onClick={onViewTemplate} data-view-standard-template>查看标准模板</button>}
    </div>}
    {!applied && serviceChanges.length > 0 && <label data-rescan-field="services" className="flex items-center gap-2 border-t border-[var(--service-diff-border)] px-4 py-2">
      <input type="checkbox" className="size-3.5 accent-primary" checked={selected.has('services')} disabled={disabled} onChange={() => onToggle('services')} />
      应用服务与端口
    </label>}
    {!applied && changeCount > 0 && rows?.filter(row => !discovery || row.changed).map(row => <div key={row.field} data-rescan-field={row.field}
      className={cn('grid grid-cols-[1rem_3.5rem_minmax(0,1fr)_0.75rem_minmax(0,1fr)] items-center gap-x-2 gap-y-1 border-t border-[var(--service-diff-border)] px-4 py-2 sm:grid-cols-[1rem_4rem_minmax(0,1fr)_0.75rem_minmax(0,1fr)_4.5rem]',
        !row.changed && 'text-muted-foreground')}>
      <input type="checkbox" aria-label={`应用${labels[row.field]}`} checked={row.changed && selected.has(row.field)}
        disabled={disabled || !row.changed} onChange={() => onToggle(row.field)} className="size-3.5 accent-primary disabled:opacity-50" />
      <span>{labels[row.field]}</span>
      <span className={cn('min-w-0 break-all font-mono text-[11px] text-muted-foreground', row.changed && 'line-through')}>{displayValue(row.field, row.before)}</span>
      {row.changed ? <ArrowRight aria-hidden="true" className="size-3 text-muted-foreground" /> : <span />}
      {row.changed ? <span className="min-w-0 max-w-full justify-self-start break-all rounded bg-success-muted px-1.5 py-0.5 font-mono text-[11px] text-success-foreground">{displayValue(row.field, row.after)}</span> : <span className="text-[11px]">无变化</span>}
      {row.changed && row.source && <span className="col-start-3 col-end-6 min-w-0 max-w-full truncate justify-self-start rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground sm:col-auto sm:justify-self-end"
        title={`${row.source}${row.confidence ? ` · ${confidenceLabels[row.confidence]}` : ''}`}>{row.source}</span>}
    </div>)}
    {!applied && <div className="flex flex-wrap items-center justify-between gap-2 border-t border-[var(--service-diff-border)] bg-background px-4 py-2.5">
      {!discovery && <p className="text-[11px] text-muted-foreground">只会填进表单，点「保存修改」才生效</p>}
      <div className="ml-auto flex gap-2">
        <Button type="button" variant="outline" size="sm" className="h-7" disabled={disabled} onClick={onDismiss}>{changeCount ? '忽略' : '关闭结果'}</Button>
        {changeCount > 0 && <Button type="button" size="sm" className="h-7" disabled={disabled || !selected.size} onClick={onApply}>应用 {selected.size} 项</Button>}
      </div>
    </div>}
  </section>;
}
