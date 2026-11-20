import { useSyncExternalStore } from 'react';
import gsap from 'gsap';
import { invertMotionBox, motionSeconds, REDUCED_MOTION_QUERY, type MotionBox } from './motionMath';

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
  const parent = node.offsetParent;
  const previous = node.style.width && parent instanceof HTMLElement ? boxIn(node, parent) : undefined;
  const inverse = previous && invertMotionBox(previous, next);
  gsap.killTweensOf(node);
  gsap.set(node, { width: next.width, height: next.height, x: next.x, y: next.y, scaleX: 1, scaleY: 1, transformOrigin: 'left top' });
  const duration = motionSeconds(seconds, reduced);
  if (!inverse || !duration) return;
  gsap.fromTo(node, { x: next.x + inverse.x, y: next.y + inverse.y, scaleX: inverse.scaleX, scaleY: inverse.scaleY },
    { x: next.x, y: next.y, scaleX: 1, scaleY: 1, duration, ease: 'power3.out' });
}
