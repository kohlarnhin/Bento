import { Fragment, useEffect, useId, useRef, useState } from 'react';
import { AlertTriangle, ArrowUpRight, Check, ChevronDown } from 'lucide-react';
import type { ProjectConfig, ServiceRuntimeStatus } from '../types';
import { injectedPortVariables, missingPortReference } from '../lib/projectServices';
import { usePortStandard } from '../lib/usePortStandard';
import { ProjectStandardDialog } from './ProjectStandardDialog';
import Button from './ui/button';

interface WebActions {
  project: ProjectConfig;
  onOpenBrowser: (port: number) => void;
  onSetDefault: (serviceId: string) => Promise<void>;
  disabled?: boolean;
  canOpen?: (serviceId: string) => boolean;
}

export function ProjectWebMenu({ project, onOpenBrowser, onSetDefault, disabled, canOpen }: WebActions) {
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const anchor = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const port = project.port;
  const services = project.services.filter(service => service.port != null);
  const defaultService = services.find(service => service.openWeb);

  const close = (restoreFocus: boolean) => {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus();
  };

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  useEffect(() => {
    if (!open) return;
    const menu = panel.current;
    (menu?.querySelector<HTMLButtonElement>('[aria-current="true"]:not(:disabled)') ?? menu?.querySelector('button:not(:disabled)'))?.focus({ preventScroll: true });
    const outside = (event: PointerEvent) => {
      if (!anchor.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [open]);

  return <div ref={anchor} className="relative" onBlur={event => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
  }}>
    <div className="project-web-split">
      <button type="button" data-project-action="open" disabled={disabled || port == null || (defaultService && canOpen?.(defaultService.id) === false)} onClick={() => { if (port != null) onOpenBrowser(port); }}
        className="rounded-l-[7px] px-[13px] py-[7px] pr-2.5 hover:bg-accent focus-visible:outline-ring disabled:opacity-50">
        打开网页{port != null && <> <span className="tabular-nums">:{port}</span></>}
      </button>
      <button ref={trigger} type="button" aria-label="选择打开网页的服务" aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined}
        disabled={disabled || !services.length} data-project-web-dropdown className="flex self-stretch items-center rounded-r-[7px] border-l border-border px-[9px] text-muted-foreground hover:bg-accent focus-visible:outline-ring disabled:opacity-50"
        onClick={() => setOpen(previous => !previous)} onKeyDown={event => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setOpen(true); }
        }}>
        <ChevronDown className="size-2.5" />
      </button>
    </div>
    {open && <div ref={panel} id={menuId} role="menu" aria-label="打开哪个端口" className="project-web-menu" data-project-web-menu
      onKeyDown={event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); return; }
        if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        const options = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
        if (!options.length) return;
        const current = options.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1
          : (current + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length;
        options[next]?.focus();
      }}>
      <div className="px-2.5 pb-1 pt-1.5 text-[11px] text-[var(--sidebar-heading)]">打开哪个端口</div>
      {services.map(service => <div key={service.id} className="project-web-option" data-default={service.openWeb || undefined}>
        <button type="button" role="menuitem" disabled={canOpen?.(service.id) === false} aria-current={service.openWeb || undefined} data-open-service={service.id}
          className="flex min-w-0 flex-1 items-center justify-between gap-2 rounded-md px-2.5 py-[7px] text-left outline-none hover:bg-accent/65 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50"
          onClick={() => { close(true); onOpenBrowser(service.port!); }}>
          <span className="truncate" title={service.name}>{service.name}{service.kind && <span className="font-normal opacity-75"> · {service.kind}</span>}</span>
          <span className="flex shrink-0 items-center gap-1 font-mono">{service.port}{service.openWeb && <Check className="size-3" aria-label="默认" />}</span>
        </button>
        {!service.openWeb && <button type="button" role="menuitem" aria-label={'设为默认：' + service.name} data-default-service={service.id}
          className="shrink-0 rounded px-1.5 py-1 text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-ring"
          onClick={() => { close(true); void onSetDefault(service.id); }}>设为默认</button>}
      </div>)}
      {services.length > 1 && <div className="mt-1 border-t border-card-border px-2.5 pb-1 pt-1.5 text-[11px] text-[var(--sidebar-heading)]">设为默认会写入项目配置</div>}
    </div>}
  </div>;
}

function statusOf(service: ServiceRuntimeStatus | undefined) {
  if (!service?.listening) return 'idle';
  return service.owner?.inProject ? 'live' : 'foreign';
}

const statusLabels = { idle: '未监听', live: '监听中', foreign: '被其他进程占用' } as const;

export function ProjectServices({ project, runtime, onOpenBrowser, onSetDefault, onEdit }: WebActions & {
  runtime?: ServiceRuntimeStatus[];
  onEdit: () => void;
}) {
  const statuses = new Map(runtime?.map(service => [service.serviceId, service]));
  const configured = project.services.filter(service => service.port != null);
  const allListening = configured.length > 0 && configured.every(service => {
    const status = statuses.get(service.id);
    return status?.port === service.port && statusOf(status) === 'live';
  });
  const vars = injectedPortVariables(project);
  const standard = usePortStandard(project.path, project.startCommand, project.services);
  const missing = new Map(standard?.data?.services
    .filter(service => !service.references.length).map(service => [service.serviceId, service.variable]));
  const [showTemplate, setShowTemplate] = useState(false);

  return <>
    <section aria-labelledby="project-services-title" className="mt-2 space-y-2.5">
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <h2 id="project-services-title" className="text-[13.5px] font-semibold leading-4">服务</h2>
        <p className="text-xs leading-4 text-muted-foreground">启动/停止由 <code className="break-all font-medium text-foreground">{project.startCommand}</code> 统一管理{allListening && '，所有端口都在监听'}</p>
        {configured.length > 0 && <button type="button" className="ml-auto shrink-0 rounded text-xs text-[var(--service-blue)] hover:underline focus-visible:outline-ring"
          onClick={() => setShowTemplate(true)} data-view-standard-template>查看标准模板</button>}
      </div>
      <div className="project-service-card overflow-x-auto">
        <table className="project-services-table" data-project-services>
          <colgroup><col className="w-1/4" /><col className="w-[19%]" /><col className="w-[15%]" /><col className="w-[19%]" /><col /></colgroup>
          <thead><tr><th scope="col">服务</th><th scope="col">目录</th><th scope="col">端口</th><th scope="col">状态</th><th scope="col">打开网页</th></tr></thead>
          <tbody>{project.services.map(service => {
            const runtime = statuses.get(service.id);
            const status = statusOf(runtime?.port === service.port ? runtime : undefined);
            const variable = missing.get(service.id);
            return <Fragment key={service.id}><tr data-service={service.id} data-service-state={status}>
              <th scope="row" className="project-service-name">
                <span>{service.name}</span>{service.openWeb && <span className="service-primary-label ml-1.5">打开网页</span>}
              </th>
              <td><code className="text-muted-foreground">{service.dir === '.' ? './' : service.dir.replace(/\/+$/, '') + '/'}</code></td>
              <td><code>{service.port ?? '—'}</code></td>
              <td><span className="project-service-status" data-state={status}
                title={status === 'foreign' && runtime?.owner?.pid != null ? 'PID ' + runtime.owner.pid : undefined}>{statusLabels[status]}</span></td>
              <td>{service.port == null ? <span className="text-muted-foreground">—</span> : service.openWeb
                ? <button type="button" className="inline-flex items-center gap-1 rounded text-[var(--service-blue)] hover:underline focus-visible:outline-ring"
                    onClick={() => onOpenBrowser(service.port!)}>localhost:{service.port}<ArrowUpRight className="size-3.5" /></button>
                : <button type="button" className="rounded text-muted-foreground hover:text-[var(--service-blue)] hover:underline focus-visible:outline-ring"
                    onClick={() => void onSetDefault(service.id)}>设为打开端口</button>}</td>
            </tr>
            {variable && <tr data-standard-warning={variable}>
              <td colSpan={5} className="!whitespace-normal !px-4 !py-2 text-xs text-warning-foreground">
                <span className="flex items-start gap-2"><AlertTriangle aria-hidden="true" className="size-3.5 shrink-0" />
                  {missingPortReference(variable)}</span>
              </td>
            </tr>}</Fragment>;
          })}</tbody>
        </table>
      </div>
      {standard?.error && <p className="text-xs text-warning-foreground" role="status">无法检查启动标准。{standard.error}</p>}
    </section>
    {showTemplate && <ProjectStandardDialog request={{ path: project.path, services: project.services, mode: 'view' }}
      onClose={() => setShowTemplate(false)} />}
    <section aria-labelledby="project-injected-vars" className="project-service-card px-4 py-3.5 text-xs leading-[1.75]" data-port-injection-summary>
      <h2 id="project-injected-vars" className="mb-1.5 text-[13.5px] font-semibold leading-normal">启动时注入的环境变量</h2>
      {project.injectPorts === false ? <div className="flex flex-wrap items-center gap-2 text-muted-foreground">
        <span>未开启端口注入</span><Button size="sm" variant="ghost" className="h-6 px-2 text-[var(--service-blue)]" onClick={onEdit}>去设置</Button>
      </div> : vars.length > 0 ? <>
        <ul className="flex flex-wrap items-center gap-x-2 gap-y-1 select-text">{vars.map(({ key, value }) =>
          <li key={key} className="min-w-0"><code className="break-all rounded bg-muted px-[5px] py-px">{key}={value}</code></li>)}</ul>
        {configured.length > 0 && <p className="mt-1 text-muted-foreground">启动脚本可以读取这些变量，使用 Bento 中配置的端口。</p>}
      </> : <p className="text-muted-foreground">自定义环境变量已提供端口设置。</p>}
    </section>
  </>;
}
