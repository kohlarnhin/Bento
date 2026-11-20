import { useLayoutEffect, useRef } from 'react';
import gsap from 'gsap';
import { Toaster as Sonner } from 'sonner';
import { useReducedMotion } from '../../lib/motion';

export function Toaster({ theme }: { theme: 'light' | 'dark' }) {
  const host = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();
  useLayoutEffect(() => {
    const node = host.current;
    if (!node || reduced) return;
    const selector = '[data-sonner-toast]';
    const seen = new WeakSet(node.querySelectorAll(selector));
    const active = new Map<HTMLElement, gsap.core.Tween>();
    const observer = new MutationObserver(() => {
      for (const toast of node.querySelectorAll<HTMLElement>(selector)) {
        if (seen.has(toast)) continue;
        seen.add(toast);
        active.set(toast, gsap.fromTo(toast, { '--toast-shift': '12px' }, {
          '--toast-shift': '0px', duration: .2, ease: 'power3.out',
          onComplete: () => { toast.style.removeProperty('--toast-shift'); active.delete(toast); },
        }));
      }
    });
    observer.observe(node, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      active.forEach((animation, toast) => {
        animation.kill();
        toast.style.removeProperty('--toast-shift');
      });
    };
  }, [reduced]);
  return <div ref={host} className="toast-host"><Sonner theme={theme} position="bottom-right" closeButton toastOptions={{
    classNames: { closeButton: 'button-neutral' },
    style: {
      background: 'var(--h1-surface)', color: 'hsl(var(--foreground))', border: '0',
      borderRadius: '16px', boxShadow: 'var(--selected-shadow)', fontFamily: 'var(--font-ui)', fontSize: '13px',
    },
  }} /></div>;
}
