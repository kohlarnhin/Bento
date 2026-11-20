import { useLayoutEffect, useRef, type HTMLAttributes } from 'react';
import gsap from 'gsap';
import { useReducedMotion } from '../lib/motion';
import { cn } from '../lib/utils';

export function StatusDot({ tone, className, ...props }: HTMLAttributes<HTMLSpanElement> & { tone: string }) {
  const dot = useRef<HTMLSpanElement>(null);
  const reduced = useReducedMotion();
  useLayoutEffect(() => {
    if (reduced || tone !== 'starting' || !dot.current) return;
    const scope = gsap.context(() => {
      gsap.fromTo(dot.current, { opacity: 1 }, { opacity: .4, duration: 1.2, yoyo: true, repeat: -1, ease: 'power2.inOut' });
    }, dot.current);
    return () => scope.revert();
  }, [reduced, tone]);
  return <span ref={dot} className={cn('h1-status-dot', className)} data-tone={tone} {...props} />;
}
