import { Fragment, useContext, useLayoutEffect } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { cn } from '../lib/utils';
import { sidebarContextPosition } from '../lib/sidebarLayout';
import { MotionExit, usePopupMotion } from './ui/motion-presence';

export interface SidebarMenuItem {
  label: string;
  title?: string;
  icon?: ReactNode;
  onSelect: () => void;
  destructive?: boolean;
  current?: boolean;
  disabled?: boolean;
  separatorBefore?: boolean;
}
export function SidebarMenu({ x, y, items, onClose, label = '侧边栏菜单', className, projectContext = false, keyboard = false }: {
  x: number; y: number; items: SidebarMenuItem[]; onClose: () => void; label?: string; className?: string; projectContext?: boolean; keyboard?: boolean;
}) {
  const onExited = useContext(MotionExit);
  const { panel: menu, attach, present } = usePopupMotion(!onExited, { onExited: onExited ?? undefined });
  useLayoutEffect(() => {
    const node = menu.current;
    if (!projectContext || !present || !node) return;
    const place = () => {
      const { left, top } = sidebarContextPosition(x, y, node.offsetWidth, node.offsetHeight, window.innerWidth, window.innerHeight);
      node.style.left = `${left}px`;
      node.style.top = `${top}px`;
    };
    place();
    const resize = new ResizeObserver(place);
    resize.observe(node);
    window.addEventListener('resize', place);
    return () => { resize.disconnect(); window.removeEventListener('resize', place); };
  }, [menu, present, projectContext, x, y]);

  useLayoutEffect(() => {
    if (onExited) return;
    const previous = document.activeElement as HTMLElement | null;
    if (!projectContext) menu.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus({ preventScroll: true });
    const outside = (event: PointerEvent) => {
      const contextClick = event.button === 2 || (event.button === 0 && event.ctrlKey);
      if (projectContext && contextClick && event.target instanceof Element &&
        event.target.closest('[data-project-row], [data-running-project]')) return;
      if (!menu.current?.contains(event.target as Node)) onClose();
    };
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) onClose(); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    document.addEventListener('pointerdown', outside, true);
    window.addEventListener('blur', onClose);
    if (projectContext) {
      document.addEventListener('scroll', scroll, true);
      document.addEventListener('wheel', scroll, { capture: true, passive: true });
      document.addEventListener('keydown', escape, true);
    }
    return () => {
      document.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('blur', onClose);
      if (projectContext) {
        document.removeEventListener('scroll', scroll, true);
        document.removeEventListener('wheel', scroll, true);
        document.removeEventListener('keydown', escape, true);
      }
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [onClose, onExited, menu, projectContext]);
  useLayoutEffect(() => {
    const node = menu.current;
    if (!projectContext || onExited || !node) return;
    const first = keyboard ? node.querySelector<HTMLButtonElement>('button:not(:disabled)') : null;
    (first ?? node).focus({ preventScroll: true });
    // WKWebView can leave programmatic focus from Shift+F10 in pointer modality.
    if (first && !first.matches(':focus-visible')) first.dataset.keyboardFocus = '';
    return () => { first?.removeAttribute('data-keyboard-focus'); };
  }, [keyboard, label, menu, onExited, projectContext, x, y]);
  if (!present) return null;
  const height = items.length * 32 + items.filter(item => item.separatorBefore).length * 9 + 16;
  return createPortal(<div ref={attach} role="menu" aria-label={label} data-sidebar-menu data-project-context={projectContext || undefined}
    tabIndex={projectContext ? -1 : undefined} data-tauri-drag-region="false" {...{ inert: onExited ? '' : undefined }}
    className={cn('h1-popup h1-menu-list fixed z-[80] min-w-40', projectContext && 'outline-none', className)}
    style={projectContext ? { left: x, top: y, maxHeight: 'calc(100vh - 16px)', overflowY: 'auto' } :
      { left: Math.max(8, Math.min(x, window.innerWidth - 190)), top: Math.max(8, Math.min(y, window.innerHeight - height)) }}
    onContextMenu={event => event.preventDefault()} onKeyDown={event => {
      if (event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); event.stopPropagation(); onClose(); }
      if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
        if (!buttons.length) return;
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
          : event.key === 'ArrowDown' ? (current + 1) % buttons.length : (Math.max(0, current) - 1 + buttons.length) % buttons.length;
        buttons[index]?.focus();
      }
    }}>
    {items.map(item => <Fragment key={item.label}>
      {item.separatorBefore && <div role="separator" className="h1-menu-divider" />}
      <button type="button" role="menuitem" title={item.title} aria-current={item.current || undefined} disabled={item.disabled}
        className={cn(item.destructive && 'text-destructive')} data-destructive={item.destructive || undefined}
        onBlur={event => { event.currentTarget.removeAttribute('data-keyboard-focus'); }}
        onPointerDown={event => { event.currentTarget.removeAttribute('data-keyboard-focus'); }}
        onClick={() => { onClose(); item.onSelect(); }}>{item.icon}{item.label}</button>
    </Fragment>)}
  </div>, document.body);
}
