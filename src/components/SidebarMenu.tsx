import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { cn } from '../lib/utils';

export interface SidebarMenuItem { label: string; icon: ReactNode; onSelect: () => void; destructive?: boolean }
export function SidebarMenu({ x, y, items, onClose }: { x: number; y: number; items: SidebarMenuItem[]; onClose: () => void }) {
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    menu.current?.querySelector<HTMLButtonElement>('button')?.focus();
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose(); };
    document.addEventListener('pointerdown', outside, true);
    window.addEventListener('blur', onClose);
    return () => {
      document.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('blur', onClose);
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [onClose]);
  return createPortal(<div ref={menu} role="menu" aria-label="侧边栏菜单" data-sidebar-menu
    className="fixed z-[80] min-w-40 rounded-lg border bg-popover p-1 text-xs text-popover-foreground shadow-lg"
    style={{ left: Math.max(8, Math.min(x, window.innerWidth - 190)), top: Math.max(8, Math.min(y, window.innerHeight - items.length * 32 - 16)) }}
    onContextMenu={event => event.preventDefault()} onKeyDown={event => {
      if (event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); event.stopPropagation(); onClose(); }
      if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button')];
        const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
        buttons[index]?.focus();
      }
    }}>
    {items.map(item => <button key={item.label} role="menuitem" className={cn('flex h-8 w-full items-center gap-2 rounded-md px-2 text-left outline-none hover:bg-accent focus:bg-accent [&_svg]:size-3.5', item.destructive && 'text-destructive')}
      onClick={() => { onClose(); item.onSelect(); }}>{item.icon}{item.label}</button>)}
  </div>, document.body);
}
