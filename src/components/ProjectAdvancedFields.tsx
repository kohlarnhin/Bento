import { ArrowDown, ArrowUp, ChevronRight, Plus, Trash2 } from 'lucide-react';
import type { EnvVar, ExtraCommand } from '../types';
import { parseEnvPaste } from '../lib/projectSettings';
import type { AdvancedErrors } from '../lib/projectSettings';
import Button from './ui/button';
import { Input } from './ui/input';
import { Textarea } from './ui/textarea';
import { Label } from './ui/label';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from './ui/collapsible';

export interface AdvancedValues { env: EnvVar[]; commands: ExtraCommand[]; autoOpen: boolean }

export function ProjectAdvancedFields({ values, onChange, errors, onBlur, open, onOpenChange, disabled, canAutoOpen }: {
  values: AdvancedValues;
  onChange: (value: Partial<AdvancedValues>) => void;
  errors: AdvancedErrors;
  onBlur: () => void;
  open: boolean;
  onOpenChange: (value: boolean) => void;
  disabled: boolean;
  canAutoOpen: boolean;
}) {
  const env = (index: number, patch: Partial<EnvVar>) => onChange({ env: values.env.map((row, position) => position === index ? { ...row, ...patch } : row) });
  const command = (index: number, patch: Partial<ExtraCommand>) => onChange({ commands: values.commands.map((row, position) => position === index ? { ...row, ...patch } : row) });
  const move = (index: number, direction: number) => {
    const commands = [...values.commands];
    [commands[index], commands[index + direction]] = [commands[index + direction], commands[index]];
    onChange({ commands });
  };
  const error = (id: string, text?: string) => text && <p id={id} className="text-[11px] leading-relaxed text-destructive" aria-live="polite">{text}</p>;
  return (
    <Collapsible open={open} onOpenChange={onOpenChange} className="rounded-lg border" data-project-advanced>
      <CollapsibleTrigger asChild><button type="button" disabled={disabled} className="group flex w-full items-center gap-2 rounded-lg px-3 py-3 text-left text-xs font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50" data-advanced-toggle><ChevronRight className="size-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-90" />高级设置<span className="ml-auto text-[11px] font-normal text-muted-foreground">环境变量 · 其他命令</span></button></CollapsibleTrigger>
      <CollapsibleContent className="space-y-5 border-t px-3 py-4">
        <section className="space-y-2" aria-label="环境变量">
          <Label>环境变量</Label>
          <p className="text-[11px] leading-relaxed text-muted-foreground">以明文保存在本地数据库中，请勿填写敏感凭据。</p>
          {values.env.map((entry, index) => <div className="space-y-2" key={index} data-env-row={index}>
            <div className="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-start gap-2">
              <Input aria-label={`变量 ${index + 1} 名称`} placeholder="KEY" value={entry.key} disabled={disabled} className="font-mono text-xs" aria-invalid={!!errors.env[index]?.key} aria-describedby={`env-${index}-error`}
                onChange={event => env(index, { key: event.target.value })} onBlur={onBlur} onPaste={event => {
                  const text = event.clipboardData.getData('text/plain');
                  if (!/[\r\n]/.test(text)) return;
                  const parsed = parseEnvPaste(text);
                  if (!parsed.length) return;
                  event.preventDefault();
                  onChange({ env: [...values.env.slice(0, index), ...parsed, ...values.env.slice(index + 1)] });
                }} />
              <Textarea aria-label={`变量 ${index + 1} 值`} placeholder="value（可为空）" rows={entry.value.includes('\n') ? 2 : 1} value={entry.value} disabled={disabled} onChange={event => env(index, { value: event.target.value })} className="min-h-9 resize-y font-mono text-xs leading-5" />
              <Button type="button" variant="ghost-destructive" size="icon" disabled={disabled} aria-label={`删除变量 ${index + 1}`} onClick={() => onChange({ env: values.env.filter((_, position) => position !== index) })}><Trash2 /></Button>
            </div>
            {error(`env-${index}-error`, errors.env[index]?.key)}
          </div>)}
          <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => onChange({ env: [...values.env, { key: '', value: '' }] })} data-add-env><Plus />添加变量</Button>
        </section>
        <section className="space-y-2" aria-label="其他命令">
          <Label>其他命令 <span className="font-normal text-muted-foreground">（最多 10 个）</span></Label>
          {values.commands.map((entry, index) => <div key={index} className="space-y-2 rounded-md border bg-muted/20 p-2" data-extra-command-row={index}>
            <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,2fr)_auto] items-start gap-2">
              <div className="min-w-0 space-y-2">
                <Input aria-label={`命令 ${index + 1} 名称`} placeholder="名称，例如 build" value={entry.name} disabled={disabled} aria-invalid={!!errors.commands[index]?.name} aria-describedby={`command-${index}-name-error`} onChange={event => command(index, { name: event.target.value })} onBlur={onBlur} />
                {error(`command-${index}-name-error`, errors.commands[index]?.name)}
              </div>
              <div className="min-w-0 space-y-2">
                <Input aria-label={`命令 ${index + 1} 内容`} placeholder="例如 pnpm build" value={entry.command} disabled={disabled} className="font-mono text-xs" aria-invalid={!!errors.commands[index]?.command} aria-describedby={`command-${index}-command-error`} onChange={event => command(index, { command: event.target.value })} onBlur={onBlur} />
                {error(`command-${index}-command-error`, errors.commands[index]?.command)}
              </div>
              <div className="flex shrink-0 pt-0.5">
                <Button type="button" variant="ghost" size="icon-sm" disabled={disabled || index === 0} aria-label={`上移命令 ${index + 1}`} onClick={() => move(index, -1)}><ArrowUp /></Button>
                <Button type="button" variant="ghost" size="icon-sm" disabled={disabled || index === values.commands.length - 1} aria-label={`下移命令 ${index + 1}`} onClick={() => move(index, 1)}><ArrowDown /></Button>
                <Button type="button" variant="ghost-destructive" size="icon-sm" disabled={disabled} aria-label={`删除命令 ${index + 1}`} onClick={() => onChange({ commands: values.commands.filter((_, position) => position !== index) })}><Trash2 /></Button>
              </div>
            </div>
          </div>)}
          {error('command-limit-error', errors.commandLimit)}
          <Button type="button" variant="outline" size="sm" disabled={disabled || values.commands.length >= 10} onClick={() => onChange({ commands: [...values.commands, { name: '', command: '' }] })} data-add-command><Plus />添加命令</Button>
        </section>
        <div className="space-y-2">
          <label className="flex items-center gap-2 text-xs"><input type="checkbox" className="size-3.5 accent-primary" checked={values.autoOpen} disabled={disabled || !canAutoOpen} onChange={event => onChange({ autoOpen: event.target.checked })} data-auto-open />启动就绪后自动打开网页</label>
          {!canAutoOpen && <p className="text-[11px] text-muted-foreground">仅填写端口的 Web 项目可用。</p>}
        </div>
        <p className="text-[11px] leading-relaxed text-muted-foreground">环境变量修改将在下次启动项目或重启终端后生效。其他命令仅在终端空闲时运行。</p>
      </CollapsibleContent>
    </Collapsible>
  );
}
