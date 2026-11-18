import { Plus, X } from 'lucide-react';
import type { DashboardConflict, PortConflictCheck } from '../types';
import { portNumber, serviceEnvName, type ServiceDraft } from '../lib/projectServices';
import Button from './ui/button';
import { Input } from './ui/input';

export type ServiceField = 'name' | 'dir' | 'port' | 'key';
export type ServiceErrors = Record<string, Partial<Record<ServiceField, string>>>;

export function ProjectServiceFields({ services, newIds, checks, errors, disabled, portFocus, onChange, onDefault, onRemove, onAdd }: {
  services: ServiceDraft[];
  newIds: ReadonlySet<string>;
  checks: Record<number, PortConflictCheck>;
  errors: ServiceErrors;
  disabled: boolean;
  portFocus?: DashboardConflict;
  onChange: (id: string, field: ServiceField, value: string) => void;
  onDefault: (id: string) => void;
  onRemove: (id: string) => void;
  onAdd: () => void;
}) {
  return <section className="space-y-[7px]" aria-labelledby="project-services-heading">
    <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-xs">
      <h3 id="project-services-heading" className="font-semibold text-[var(--sidebar-muted)]">服务与端口</h3>
      <span className="text-muted-foreground">所有已配置端口都会监测，单选的端口用于「打开网页」</span>
    </div>
    <div className="overflow-x-auto">
      <table className="project-service-editor" data-project-service-table>
        <thead>
          <tr>
            <th scope="col">名称</th><th scope="col">目录</th><th scope="col">端口</th><th scope="col">环境变量</th>
            <th scope="col" className="text-center">打开网页</th><th scope="col"><span className="sr-only">操作</span></th>
          </tr>
        </thead>
        {services.map((service, index) => {
          const name = service.name || '服务 ' + (index + 1);
          const validation = errors[service.id];
          const port = portNumber(service.port);
          const result = checks[port ?? 0];
          const unavailable = !!(result?.inUse || result?.reserved.length || validation?.port);
          const initialSuggestion = portFocus?.serviceId === service.id && portFocus.port === port ? portFocus.suggestion : null;
          const suggestion = (unavailable ? result?.suggestion : null) ?? initialSuggestion;
          return <tbody key={service.id} data-project-service-row={service.id}>
            <tr>
              <td><div className="flex items-center gap-1.5">
                <Input aria-label={name + '的名称'} placeholder="服务名称" value={service.name} aria-invalid={!!validation?.name}
                  title={validation?.name} disabled={disabled} onChange={event => onChange(service.id, 'name', event.target.value)} />
                {newIds.has(service.id) && <span className="service-primary-label" data-new-service>新</span>}
              </div></td>
              <td><Input aria-label={name + '的目录'} placeholder="相对目录" value={service.dir} aria-invalid={!!validation?.dir}
                title={validation?.dir} className="font-mono" disabled={disabled} onChange={event => onChange(service.id, 'dir', event.target.value)} /></td>
              <td><Input aria-label={name + '的端口'} placeholder="端口" value={service.port} inputMode="numeric" data-service-port-input={service.id}
                aria-invalid={!!validation?.port} title={validation?.port} className="font-mono" disabled={disabled}
                onChange={event => onChange(service.id, 'port', event.target.value)} /></td>
              <td>
                <Input aria-label={name + '的角色 key'} placeholder="例如 WEB" value={service.key} aria-invalid={!!validation?.key}
                  title={validation?.key} className="max-w-[114px] font-mono" disabled={disabled}
                  autoCapitalize="characters" spellCheck={false} onChange={event => onChange(service.id, 'key', event.target.value)} />
                <code className="mt-1 block text-meta text-[var(--sidebar-muted)]">
                  {services.length === 1 ? 'PORT' : service.key ? serviceEnvName(service.key) : '—'}
                </code>
              </td>
              <td className="text-center"><input type="radio" name="open-web-service" aria-label={name + '用于打开网页'} checked={service.openWeb}
                className="project-service-radio" disabled={disabled} onChange={() => onDefault(service.id)} /></td>
              <td><Button type="button" variant="ghost-destructive" size="icon-sm" aria-label={'删除' + name} disabled={disabled}
                onClick={() => onRemove(service.id)}><X className="!size-3.5" /></Button></td>
            </tr>
            {(validation?.name || validation?.dir || validation?.key || unavailable || suggestion != null) && <tr>
              <td colSpan={6} className="!border-t-0 px-2 !pb-2 !pt-0 text-[11px]" role="status">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  {Object.values(validation ?? {}).map(error => <span key={error} className="text-destructive">{error}</span>)}
                  {result?.inUse && <span className="text-warning-foreground">端口正被其他进程监听{result.inUse.owners.length > 0
                    ? '（PID ' + result.inUse.owners.map(owner => owner.pid).join('、') + '）' : ''}，仍可保存。</span>}
                  {result?.reserved.map(message => <span key={message} className="text-warning-foreground">{message}</span>)}
                  {suggestion != null && <Button type="button" variant="outline" size="sm" className="h-6 text-[11px]"
                    disabled={disabled} onClick={() => onChange(service.id, 'port', String(suggestion))}>改用 {suggestion}</Button>}
                </div>
              </td>
            </tr>}
          </tbody>;
        })}
      </table>
    </div>
    <button type="button" className="inline-flex items-center gap-1 rounded px-2 pt-0.5 text-[12.5px] text-[var(--service-blue)] hover:underline focus-visible:outline-ring disabled:opacity-50"
      disabled={disabled} onClick={onAdd} data-add-service><Plus className="size-3.5" />添加服务</button>
  </section>;
}
