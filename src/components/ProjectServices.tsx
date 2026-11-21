import { useEffect, useId, useRef, useState, type ComponentPropsWithoutRef } from 'react';
import { ArrowUpRight, Check, ChevronDown } from 'lucide-react';
import type { ProjectConfig, Service } from '../types';
import { ActionTooltip } from './ui/tooltip';
import { usePopupMotion } from './ui/motion-presence';
import { cn } from '../lib/utils';

interface WebActions {
  project: ProjectConfig;
  onOpenBrowser: (port: number) => void;
  onSetDefault: (serviceId: string) => Promise<void>;
}
export function OpenServiceButton({ port, listening, onOpen, wrapperClassName, ...button }: Omit<
  ComponentPropsWithoutRef<'button'>, 'type' | 'disabled' | 'onClick'
> & {
  port?: number | null;
  listening: boolean;
  onOpen: (port: number) => void;
  wrapperClassName?: string;
}) {
  const reason = port == null ? '未配置服务端口' : listening ? undefined : '端口 ' + port + ' 尚未监听';
  return <ActionTooltip label={reason}>
    <span className={cn('inline-flex', wrapperClassName)} title={reason} aria-label={reason} tabIndex={reason ? 0 : undefined}>
      <button {...button} type="button" disabled={!!reason} onClick={() => { if (port != null && listening) onOpen(port); }} />
    </span>
  </ActionTooltip>;
}

export function ProjectWebMenu({ project, onOpenBrowser, onSetDefault, listeningPort, actualPort, terminal }: WebActions & {
  listeningPort: (service: Service) => number | null;
  actualPort: number | null;
  terminal: boolean;
}) {
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const anchor = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const { panel, attach, present } = usePopupMotion(open);
  const port = actualPort ?? project.port;
  const services = project.services.map(service => ({ service, listening: listeningPort(service) }))
    .filter(({ service, listening }) => service.port != null || listening != null);

  const close = (restoreFocus: boolean) => {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    const menu = panel.current;
    (menu?.querySelector<HTMLButtonElement>('[aria-current="true"]:not(:disabled)') ?? menu?.querySelector('button:not(:disabled)'))?.focus({ preventScroll: true });
    const outside = (event: PointerEvent) => {
      if (!anchor.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [open, panel]);

  return <div ref={anchor} className="relative" onBlur={event => {
    // WebKit can blur the focused item on mouse-down without focusing the clicked item.
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) setOpen(false);
  }}>
    <div className="project-web-split">
      <OpenServiceButton data-project-action="open" port={port} listening={actualPort != null} onOpen={onOpenBrowser}
        className="project-web-main" title={port == null ? undefined : 'localhost:' + port}>
        打开网页 <ArrowUpRight size={13} aria-hidden="true" />
      </OpenServiceButton>
      <button ref={trigger} type="button" aria-label="选择打开网页的服务" aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined}
        disabled={!services.length} data-project-web-dropdown className="project-web-trigger"
        onClick={() => setOpen(previous => !previous)} onKeyDown={event => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setOpen(true); }
        }}>
        <ChevronDown className="size-2.5" />
      </button>
    </div>
    {present && <div ref={attach} id={menuId} role="menu" aria-label="打开哪个端口" className={cn('h1-popup project-web-menu', terminal && 'terminal-chrome')} data-project-web-menu
      data-tauri-drag-region="false"
      {...{ inert: open ? undefined : '' }}
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
      <div className="px-2.5 pb-1 pt-1.5 text-[11px] text-muted-foreground">打开哪个端口</div>
      {services.map(({ service, listening }) => <div key={service.id} className="project-web-option" data-default={service.openWeb || undefined}>
        <OpenServiceButton role="menuitem" port={listening ?? service.port} listening={listening != null} aria-current={service.openWeb || undefined} data-open-service={service.id}
          wrapperClassName="min-w-0 flex-1" className="choice-row flex w-full min-w-0 items-center justify-between gap-2 rounded-md px-2.5 py-[7px] text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/25 disabled:opacity-50"
          onOpen={port => { close(true); onOpenBrowser(port); }}>
          <span className="truncate" title={service.name}>{service.name}{service.kind && <span className="font-normal opacity-75"> · {service.kind}</span>}</span>
          <span className="flex shrink-0 items-center gap-1 font-mono">{listening ?? service.port}{service.openWeb && <Check className="size-3" aria-label="默认" />}</span>
        </OpenServiceButton>
        {!service.openWeb && <button type="button" role="menuitem" aria-label={'设为默认：' + service.name} data-default-service={service.id}
          className="h1-tag shrink-0 !text-[10px]"
          onClick={() => { close(true); void onSetDefault(service.id); }}>设为默认</button>}
      </div>)}
      {services.length > 1 && <div className="mt-1 border-t border-card-border px-2.5 pb-1 pt-1.5 text-[11px] text-muted-foreground">设为默认会写入项目配置</div>}
    </div>}
  </div>;
}
