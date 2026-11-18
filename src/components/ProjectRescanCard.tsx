import { AlertCircle, ArrowRight, Check } from 'lucide-react';
import type { RescanField, RescanRow } from '../lib/projectRescan';
import { cn } from '../lib/utils';
import Button from './ui/button';

const labels = { startCommand: '启动命令', port: '端口', type: '类型' };
const confidenceLabels = { high: '高置信度', medium: '中置信度', low: '低置信度' };
const displayValue = (field: RescanField, value: string) => field === 'type' ? (value === 'app' ? '桌面应用' : 'Web 项目') : value.trim() || '未填写';

export function ProjectRescanCard({ rows, sourceFiles = [], selected, error, disabled, onToggle, onDismiss, onApply }: {
  rows?: RescanRow[];
  sourceFiles?: string[];
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
  const changeCount = rows?.filter(row => row.changed).length ?? 0;
  if (!changeCount) return <div role="status" className="flex items-center gap-2 rounded-lg border border-success/15 bg-success-muted px-3.5 py-3 text-xs text-success-foreground">
    <Check aria-hidden="true" className="size-4 shrink-0" />配置已是最新
  </div>;

  return <section aria-label="重新扫描结果" className="overflow-hidden rounded-lg border border-blue-100 bg-blue-50/55 text-xs">
    <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 px-3.5 py-2.5">
      <h4 role="status" className="shrink-0 font-semibold text-blue-800">发现 {changeCount} 处变化</h4>
      {!!sourceFiles.length && <span className="min-w-0 break-words text-[11px] text-muted-foreground">来自 {sourceFiles.join(' · ')}</span>}
    </div>
    {rows?.map(row => <div key={row.field} data-rescan-field={row.field}
      className={cn('grid grid-cols-[1rem_3.5rem_minmax(0,1fr)_0.75rem_minmax(0,1fr)] items-center gap-x-2 gap-y-1 border-t border-blue-100/80 px-3.5 py-2 sm:grid-cols-[1rem_4rem_minmax(0,1fr)_0.75rem_minmax(0,1fr)_4.5rem]',
        !row.changed && 'text-muted-foreground')}>
      <input type="checkbox" aria-label={`应用${labels[row.field]}`} checked={row.changed && selected.has(row.field)}
        disabled={disabled || !row.changed} onChange={() => onToggle(row.field)} className="size-3.5 accent-primary disabled:opacity-50" />
      <span>{labels[row.field]}</span>
      <span className={cn('min-w-0 break-all font-mono text-[11px] text-muted-foreground', row.changed && 'line-through')}>{displayValue(row.field, row.before)}</span>
      {row.changed ? <ArrowRight aria-hidden="true" className="size-3 text-muted-foreground" /> : <span />}
      {row.changed ? <span className="min-w-0 max-w-full justify-self-start break-all rounded bg-emerald-50 px-1.5 py-0.5 font-mono text-[11px] text-emerald-800">{displayValue(row.field, row.after)}</span> : <span className="text-[11px]">无变化</span>}
      {row.changed && row.source && <span className="col-start-3 col-end-6 min-w-0 max-w-full truncate justify-self-start rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground sm:col-auto sm:justify-self-end"
        title={`${row.source}${row.confidence ? ` · ${confidenceLabels[row.confidence]}` : ''}`}>{row.source}</span>}
    </div>)}
    <div className="flex flex-wrap items-center justify-between gap-2 border-t border-blue-100/80 bg-background px-3.5 py-2.5">
      <p className="text-[11px] text-muted-foreground">只会填进表单，点「保存修改」才生效</p>
      <div className="ml-auto flex gap-2">
        <Button type="button" variant="outline" size="sm" className="h-7" disabled={disabled} onClick={onDismiss}>忽略</Button>
        <Button type="button" size="sm" className="h-7" disabled={disabled || !selected.size} onClick={onApply}>应用 {selected.size} 项</Button>
      </div>
    </div>
  </section>;
}
