import { createContext, useCallback, useLayoutEffect, useRef, useState, type ReactElement, type RefObject } from 'react';
import gsap from 'gsap';
import { useReducedMotion } from '../../lib/motion';

export const MotionExit = createContext<(() => void) | null>(null);

/** Keep the most recent children mounted until their closing animation finishes. */
export function MotionPresence({ children }: { children: ReactElement | null | false | undefined }) {
  const [retained, setRetained] = useState(children);
  if (children && children !== retained) setRetained(children);
  const remove = useCallback(() => setRetained(null), []);
  return <MotionExit.Provider value={children ? null : remove}>{children || retained}</MotionExit.Provider>;
}

export function usePopupMotion(open: boolean, options?: { backdrop?: RefObject<HTMLElement>; onExited?: () => void }) {
  const panel = useRef<HTMLDivElement | null>(null);
  const [node, setNode] = useState<HTMLDivElement | null>(null);
  const attach = useCallback((element: HTMLDivElement | null) => {
    panel.current = element;
    setNode(element);
  }, []);
  const [retained, setRetained] = useState(open);
  const reduced = useReducedMotion();
  if (open && !retained) setRetained(true);
  const present = open || retained;
  const backdrop = options?.backdrop;
  const onExited = useRef(options?.onExited);
  onExited.current = options?.onExited;
  const previous = useRef<{ node: HTMLElement; opacity: number; scale: number; shade: number }>();

  useLayoutEffect(() => {
    const shade = backdrop?.current;
    if (!present || !node) return;
    const complete = () => {
      if (!open) { setRetained(false); onExited.current?.(); }
    };
    if (reduced) { complete(); return; }
    const actual = previous.current?.node === node ? previous.current : undefined;
    const scope = gsap.context(() => {
      const duration = open ? .18 : .12;
      const ease = open ? 'power3.out' : 'power2.inOut';
      if (shade) gsap.fromTo(shade, { opacity: actual?.shade ?? 0 },
        { opacity: open ? 1 : 0, duration, ease, ...(open && { clearProps: 'opacity' }) });
      gsap.fromTo(node, { opacity: actual?.opacity ?? 0, '--popup-scale': actual?.scale ?? .96 }, {
        opacity: open ? 1 : 0, '--popup-scale': open ? 1 : .96, duration, ease,
        ...(open && { clearProps: 'opacity,--popup-scale' }), onComplete: complete,
      });
    }, node);
    return () => {
      previous.current = { node, opacity: Number(getComputedStyle(node).opacity),
        scale: Number(node.style.getPropertyValue('--popup-scale') || 1), shade: shade ? Number(getComputedStyle(shade).opacity) : 1 };
      scope.revert();
    };
  }, [backdrop, node, open, present, reduced]);
  return { panel, attach, present };
}
