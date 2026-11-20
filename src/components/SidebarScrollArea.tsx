import { createContext, forwardRef, useCallback, useContext, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { sidebarThumb } from '../lib/sidebarLayout';
import { cn } from '../lib/utils';

const ScrollbarContext = createContext<{
  active: string | undefined;
  visible: boolean;
  reveal: (id: string, held: boolean) => void;
} | null>(null);

export const SidebarScrollArea = forwardRef<HTMLDivElement, { className?: string; children: ReactNode }>(
  function SidebarScrollArea({ className, children }, ref) {
    const parentScrollbar = useContext(ScrollbarContext);
    const id = useId();
    const viewport = useRef<HTMLDivElement>(null);
    const content = useRef<HTMLDivElement>(null);
    const hideTimer = useRef<ReturnType<typeof setTimeout>>();
    const drag = useRef<{ pointerId: number; y: number; scrollTop: number; scrollPerPixel: number }>();
    const [thumb, setThumb] = useState<ReturnType<typeof sidebarThumb>>();
    const [active, setActive] = useState<string>();
    const [visible, setVisible] = useState(false);
    const showThumb = useCallback((next: string, held: boolean) => {
      setActive(next);
      setVisible(true);
      clearTimeout(hideTimer.current);
      if (!held) hideTimer.current = setTimeout(() => setVisible(false), 1000);
    }, []);
    // Both the running section and its parent scroll in the same right-side channel.
    const scrollbar = useMemo(() => parentScrollbar ?? { active, visible, reveal: showThumb },
      [parentScrollbar, active, visible, showThumb]);

    const measure = (element: HTMLDivElement) => {
      setThumb(sidebarThumb(element.clientHeight, element.scrollHeight, element.scrollTop));
    };
    useLayoutEffect(() => {
      const element = viewport.current;
      if (!element || !content.current) return;
      const observer = new ResizeObserver(() => measure(element));
      observer.observe(element);
      observer.observe(content.current);
      measure(element);
      return () => { observer.disconnect(); clearTimeout(hideTimer.current); };
    }, []);

    const reveal = () => {
      const element = viewport.current;
      if (element && element.scrollHeight > element.clientHeight) scrollbar.reveal(id, !!drag.current);
    };
    const endDrag = () => {
      if (!drag.current) return;
      drag.current = undefined;
      reveal();
    };

    const activeThumb = scrollbar.active === id;
    return <ScrollbarContext.Provider value={scrollbar}>
      <div ref={ref} className={cn('sidebar-scroll-area', className)} onMouseEnter={reveal}>
        <div ref={viewport} className="sidebar-scroll-viewport" data-sidebar-scroll-viewport
          onScroll={event => { measure(event.currentTarget); reveal(); }}>
          <div ref={content}>{children}</div>
        </div>
        {thumb && <div className="sidebar-scroll-thumb" aria-hidden="true" data-active={activeThumb || undefined}
          data-visible={activeThumb && scrollbar.visible || undefined}
          style={{ height: thumb.height, transform: `translateY(${thumb.top}px)` }}
          onPointerDown={event => {
            const element = viewport.current;
            if (event.button !== 0 || !element) return;
            event.preventDefault();
            drag.current = { pointerId: event.pointerId, y: event.clientY, scrollTop: element.scrollTop, scrollPerPixel: thumb.scrollPerPixel };
            event.currentTarget.setPointerCapture(event.pointerId);
            reveal();
          }}
          onPointerMove={event => {
            const start = drag.current;
            if (!start || start.pointerId !== event.pointerId || !viewport.current) return;
            viewport.current.scrollTop = start.scrollTop + (event.clientY - start.y) * start.scrollPerPixel;
          }}
          onPointerUp={endDrag} onPointerCancel={endDrag} onLostPointerCapture={endDrag} />}
      </div>
    </ScrollbarContext.Provider>;
  },
);
