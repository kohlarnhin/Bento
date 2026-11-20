import { useLayoutEffect, useRef, type RefObject } from 'react';
import gsap from 'gsap';
import { useReducedMotion } from '../lib/motion';
import { fitVisibleTerminals } from '../lib/terminalRegistry';

export function useWindowMotion(root: RefObject<HTMLDivElement>, focused: boolean) {
  const reduced = useReducedMotion();
  const introDone = useRef(false);
  useLayoutEffect(() => {
    if (introDone.current || !root.current) return;
    if (reduced) { introDone.current = true; return; }
    const scope = gsap.context(() => {
      const panels = gsap.utils.toArray<HTMLElement>('.h1-sidebar-panel, .h1-main');
      const intro = gsap.timeline({ onComplete: () => { introDone.current = true; } });
      const timing = { duration: .42, stagger: .06, ease: 'back.out(1.4)' };
      // Anchor the inner edges so the two painted panels never cover the seam.
      intro.fromTo(panels, { scale: .985, opacity: 0, transformOrigin: (i: number) => i === 0 ? 'right center' : 'left center' },
        { scale: 1, opacity: 1, ...timing, clearProps: 'opacity,transform,transformOrigin' }, 0);
      const contents = panels.map(panel => panel.querySelector(':scope > :not([data-tauri-drag-region])')).filter(node => node instanceof HTMLElement);
      intro.fromTo(contents, { x: (i: number) => i === 0 ? -16 : 16 },
        { x: 0, ...timing, clearProps: 'transform' }, 0);
    }, root.current);
    return () => scope.revert();
  }, [root, reduced]);

  const settled = useRef(focused);
  const layout = useRef<{ revert: () => void; timeline: gsap.core.Timeline }>();
  useLayoutEffect(() => () => {
    layout.current?.revert();
    layout.current = undefined;
  }, []);
  useLayoutEffect(() => {
    const surface = root.current;
    if (!surface) return;
    const finish = (next: boolean) => {
      settled.current = next;
      const active = layout.current;
      layout.current = undefined;
      active?.revert();
      delete surface.dataset.terminalLayoutMotion;
      fitVisibleTerminals();
    };
    const active = layout.current;
    if (reduced) {
      if (active || settled.current !== focused) finish(focused);
      return;
    }
    if (active) {
      if (active.timeline.progress() === Number(focused)) finish(focused);
      else active.timeline.reversed(!focused).resume();
      return;
    }
    if (settled.current === focused) return;

    const expanded = surface.querySelector<HTMLElement>('.app-sidebar')!;
    const rail = surface.querySelector<HTMLElement>('.terminal-focus-rail')!;
    const names = expanded.querySelectorAll('.sidebar-search-wrap, .sidebar-group-heading, .sidebar-project-row, .sidebar-footer');
    const icons = rail.querySelectorAll('.terminal-focus-project, .terminal-focus-settings');
    const css = getComputedStyle(surface);
    const wide = parseFloat(css.getPropertyValue('--sidebar-expanded-width'));
    const narrow = parseFloat(css.getPropertyValue('--sidebar-compact-width'));
    const width = { px: wide };
    let resizeStart = 0;
    let resizeEnd = 0;
    let timeline!: gsap.core.Timeline;
    const applyLayout = () => {
      const time = timeline.time();
      surface.style.setProperty('--motion-sidebar-width', Math.round(width.px) + 'px');
      expanded.style.visibility = time < resizeStart ? 'visible' : 'hidden';
      rail.style.visibility = time >= resizeEnd ? 'visible' : 'hidden';
    };
    const scope = gsap.context(() => {
      timeline = gsap.timeline({ paused: true, onUpdate: applyLayout, onComplete: () => finish(true), onReverseComplete: () => finish(false) });
      timeline.fromTo(names, { x: 0, opacity: 1 },
        { x: -6, opacity: 0, duration: .12, stagger: (index: number) => .02 * Math.min(index, 4), ease: 'power2.inOut' });
      resizeStart = timeline.duration();
      timeline.fromTo(width, { px: wide }, { px: narrow, duration: .32, ease: 'power2.inOut' });
      resizeEnd = timeline.duration();
      timeline.fromTo(icons, { scale: .8, opacity: 0 },
        { scale: 1, opacity: 1, duration: .15, stagger: (index: number) => .025 * Math.min(index, 4), ease: 'power3.out' });
    }, surface);
    surface.dataset.terminalLayoutMotion = '';
    layout.current = { timeline, revert: () => {
      scope.revert();
      surface.style.removeProperty('--motion-sidebar-width');
      expanded.style.removeProperty('visibility');
      rail.style.removeProperty('visibility');
      delete surface.dataset.terminalLayoutMotion;
    } };
    timeline.progress(focused ? 0 : 1, true);
    applyLayout();
    if (focused) timeline.play();
    else timeline.reverse();
  }, [root, focused, reduced]);
}
