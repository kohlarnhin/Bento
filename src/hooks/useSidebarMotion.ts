import { useLayoutEffect, useRef, type RefObject } from 'react';
import gsap from 'gsap';
import { boxIn, moveIndicator, useReducedMotion } from '../lib/motion';
import type { MotionBox } from '../lib/motionMath';
import type { ProjectConfig } from '../types';

export function useSidebarMotion(list: RefObject<HTMLElement>, marker: RefObject<HTMLSpanElement>,
  projects: ProjectConfig[], loaded: boolean, inactive: boolean) {
  const reduced = useReducedMotion();
  const known = useRef<Set<string>>();
  useLayoutEffect(() => {
    if (!loaded) return;
    const before = known.current;
    known.current = new Set(projects.map(project => project.id));
    if (!before || reduced || inactive || !list.current) return;
    const added = [...list.current.querySelectorAll<HTMLElement>('[data-project-id]')]
      .filter(row => !before.has(row.dataset.projectId!));
    if (!added.length) return;
    const scope = gsap.context(() => {
      const heights = added.map(row => row.offsetHeight);
      gsap.fromTo(added, { height: 0, opacity: 0, overflow: 'hidden' }, {
        height: (index: number) => heights[index], opacity: 1, duration: .22, ease: 'power3.out',
        clearProps: 'height,opacity,overflow',
      });
    }, list.current);
    return () => scope.revert();
  }, [list, projects, loaded, reduced, inactive]);

  const last = useRef<{ row: HTMLElement; box: MotionBox; reduced: boolean }>();
  const measure = useRef<(animate: boolean) => void>();
  useLayoutEffect(() => {
    measure.current = animate => {
      const parent = list.current;
      const outline = marker.current;
      if (!parent || !outline) return;
      const row = inactive ? undefined : [...parent.querySelectorAll<HTMLElement>('.sidebar-project-surface[data-selected]:not([data-highlighted])')]
        .find(candidate => {
          const viewport = candidate.closest('[data-radix-scroll-area-viewport]');
          if (!viewport) return true;
          const item = candidate.getBoundingClientRect();
          const bounds = viewport.getBoundingClientRect();
          return item.height > 0 && item.top >= bounds.top - 1 && item.bottom <= bounds.bottom + 1;
        });
      const previous = last.current;
      if (!row) {
        previous?.row.removeAttribute('data-motion-selected');
        last.current = undefined;
        outline.hidden = true;
        gsap.killTweensOf(outline);
        outline.style.removeProperty('width');
        return;
      }
      const box = boxIn(row, parent);
      if (previous?.row === row && previous.reduced === reduced &&
        previous.box.x === box.x && previous.box.y === box.y && previous.box.width === box.width && previous.box.height === box.height) return;
      previous?.row.removeAttribute('data-motion-selected');
      row.dataset.motionSelected = '';
      outline.hidden = false;
      last.current = { row, box, reduced };
      moveIndicator(outline, box, animate && previous?.row !== row ? .22 : 0, reduced);
    };
    measure.current(true);
  });

  useLayoutEffect(() => {
    const parent = list.current;
    const outline = marker.current;
    if (!parent || !outline) return;
    const refresh = () => measure.current?.(false);
    const resize = new ResizeObserver(refresh);
    resize.observe(parent);
    parent.closest('.sidebar-scroll')?.addEventListener('scroll', refresh, true);
    return () => {
      resize.disconnect();
      parent.closest('.sidebar-scroll')?.removeEventListener('scroll', refresh, true);
      last.current?.row.removeAttribute('data-motion-selected');
      last.current = undefined;
      gsap.killTweensOf(outline);
    };
  }, [list, marker]);
}
