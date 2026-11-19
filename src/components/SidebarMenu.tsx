import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { cn } from '../lib/utils';

export interface SidebarMenuItem { label: string; icon: ReactNode; onSelect: () => void; destructive?: boolean; current?: boolean; disabled?: boolean }
export function SidebarMenu({ x, y, items, onClose, label = '侧边栏菜单', className }: {
  x: number; y: number; items: SidebarMenuItem[]; onClose: () => void; label?: string; className?: string;
}) {
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    menu.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', outside, true);
    window.addEventListener('blur', onClose);
    return () => {
      document.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('blur', onClose);
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [onClose]);
  return createPortal(<div ref={menu} role="menu" aria-label={label} data-sidebar-menu
    className={cn('h1-popup h1-menu-list fixed z-[80] min-w-40', className)}
    style={{ left: Math.max(8, Math.min(x, window.innerWidth - 190)), top: Math.max(8, Math.min(y, window.innerHeight - items.length * 32 - 16)) }}
    onContextMenu={event => event.preventDefault()} onKeyDown={event => {
      if (event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); event.stopPropagation(); onClose(); }
      if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
        if (!buttons.length) return;
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
        buttons[index]?.focus();
      }
    }}>
    {items.map(item => <button key={item.label} role="menuitem" aria-current={item.current || undefined} disabled={item.disabled} className={cn(item.destructive && 'text-destructive')}
      onClick={() => { onClose(); item.onSelect(); }}>{item.icon}{item.label}</button>)}
  </div>, document.body);
}
