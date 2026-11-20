export const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

export function motionSeconds(seconds: number, reduced: boolean) {
  return reduced ? 0 : seconds;
}

export interface MotionBox { x: number; y: number; width: number; height: number }

/** Invert the final box around its top-left corner to start at the previous painted box. */
export function invertMotionBox(previous: MotionBox, next: MotionBox) {
  if (previous.width <= 0 || previous.height <= 0 || next.width <= 0 || next.height <= 0) return;
  return { x: previous.x - next.x, y: previous.y - next.y,
    scaleX: previous.width / next.width, scaleY: previous.height / next.height };
}

export const dashboardEntranceLimit = 8;

export interface TerminalTabPaint { backdrop: number; content: number; y: number }

export function settledTerminalTab(terminal: boolean): TerminalTabPaint {
  return terminal ? { backdrop: 1, content: 1, y: 0 } : { backdrop: 0, content: 0, y: 4 };
}

export function animateTerminalTab(timeline: gsap.core.Timeline, paint: TerminalTabPaint, terminal: boolean) {
  if (terminal) {
    if (paint.content === 0) paint.y = 4;
    if (paint.backdrop < 1) timeline.to(paint, { backdrop: 1, duration: .12 * (1 - paint.backdrop), ease: 'power2.out' });
    if (paint.content < 1) timeline.to(paint, { content: 1, y: 0, duration: .16 * (1 - paint.content), ease: 'power3.out' });
  } else {
    if (paint.content > 0) timeline.to(paint, { content: 0, duration: .1 * paint.content, ease: 'power2.out' });
    if (paint.backdrop > 0) timeline.to(paint, { backdrop: 0, duration: .12 * paint.backdrop, ease: 'power2.out' });
  }
}
