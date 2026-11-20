import type { ProjectView } from '../types.ts';

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

export const projectViewOrder = ['overview', 'git', 'terminal'] as const;
export const projectViewBlendSeconds = .2;
export const projectViewContentTiming = { enter: .06, exit: .09 } as const;
export type ProjectViewMix = Record<ProjectView, number>;

export function advanceProjectViewTime(elapsed: number, frameSeconds: number) {
  // A slow frame should extend the blend instead of skipping a large brightness step.
  return Math.min(projectViewBlendSeconds, elapsed + Math.min(frameSeconds, .024));
}

export function readyProjectView(request: ProjectView, shown: ProjectView | undefined, terminalReady: boolean, gitReady: boolean) {
  // A newly selected project has no previous surface to keep while its content starts.
  if (shown && ((request === 'terminal' && !terminalReady) || (request === 'git' && !gitReady))) return shown;
  return request;
}

export function blendProjectViews(from: ProjectViewMix, to: ProjectView, progress: number): ProjectViewMix {
  return {
    overview: from.overview * (1 - progress) + (to === 'overview' ? progress : 0),
    git: from.git * (1 - progress) + (to === 'git' ? progress : 0),
    terminal: from.terminal * (1 - progress) + (to === 'terminal' ? progress : 0),
  };
}

/** Convert visual shares to source-over opacity, from the bottom view to the top. */
export function projectViewOpacities(mix: ProjectViewMix): ProjectViewMix {
  const lower = mix.overview + mix.git;
  return { overview: Number(mix.overview > 0), git: lower ? mix.git / lower : 0, terminal: mix.terminal };
}

/** Fade foregrounds on their own schedule while the opaque surfaces keep blending. */
export function projectViewContentOpacities(from: ProjectViewMix, to: ProjectView, seconds: number): ProjectViewMix {
  const { enter, exit } = projectViewContentTiming;
  const ease = (progress: number) => Math.sin(Math.max(0, Math.min(1, progress)) * Math.PI / 2);
  const leaving = 1 - ease(seconds / exit);
  const entering = ease((seconds - enter) / (projectViewBlendSeconds - enter));
  const opacity = (view: ProjectView) => view === to ? from[view] + (1 - from[view]) * entering : from[view] * leaving;
  return { overview: opacity('overview'), git: opacity('git'), terminal: opacity('terminal') };
}
