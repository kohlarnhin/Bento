import { useSyncExternalStore } from 'react';
import gsap from 'gsap';
import { motionSeconds, REDUCED_MOTION_QUERY, type MotionBox } from './motionMath';

const listeners = new Set<() => void>();
let media: ReturnType<typeof gsap.matchMedia> | undefined;
const prefersReducedMotion = () => window.matchMedia(REDUCED_MOTION_QUERY).matches;

function subscribeToMotion(listener: () => void) {
  listeners.add(listener);
  if (!media) {
    media = gsap.matchMedia();
    media.add({ all: 'all', reduced: REDUCED_MOTION_QUERY }, () => listeners.forEach(notify => notify()));
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      media?.revert();
      media = undefined;
    }
  };
}

export function useReducedMotion() {
  return useSyncExternalStore(subscribeToMotion, prefersReducedMotion, () => true);
}

/** Both boxes use the indicator's offset parent's coordinate space. */
export function boxIn(element: HTMLElement, parent: HTMLElement): MotionBox {
  const box = element.getBoundingClientRect();
  const bounds = parent.getBoundingClientRect();
  const sx = bounds.width / parent.offsetWidth || 1;
  const sy = bounds.height / parent.offsetHeight || 1;
  return {
    x: (box.left - bounds.left) / sx + parent.scrollLeft - parent.clientLeft,
    y: (box.top - bounds.top) / sy + parent.scrollTop - parent.clientTop,
    width: box.width / sx, height: box.height / sy,
  };
}

export function moveIndicator(node: HTMLElement, next: MotionBox, seconds: number, reduced: boolean) {
  // Slide the thumb: tween position and size directly (no scale), so the pill keeps its rounded ends
  // while it glides from the old segment to the new one.
  const placed = Boolean(node.style.width);
  gsap.killTweensOf(node);
  const duration = motionSeconds(seconds, reduced);
  if (!placed || !duration) {
    gsap.set(node, { width: next.width, height: next.height, x: next.x, y: next.y, scaleX: 1, scaleY: 1, transformOrigin: 'left top' });
    return;
  }
  gsap.set(node, { scaleX: 1, scaleY: 1 });
  gsap.to(node, { x: next.x, y: next.y, width: next.width, height: next.height, duration, ease: 'power3.out', overwrite: true });
}
