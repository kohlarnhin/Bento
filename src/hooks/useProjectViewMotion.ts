import { useLayoutEffect, useRef, type RefObject } from 'react';
import gsap from 'gsap';
import type { ProjectView } from '../types';
import {
  advanceProjectViewTime, blendProjectViews, projectViewBlendSeconds, projectViewContentOpacities, projectViewOpacities, projectViewOrder,
} from '../lib/motionMath';

const ease = gsap.parseEase('sine.out');

export function useProjectViewMotion(workspace: RefObject<HTMLDivElement>, view: ProjectView, reduced: boolean) {
  const initial = blendProjectViews({ overview: 0, git: 0, terminal: 0 }, view, 1);
  const painted = useRef({ surface: initial, content: initial });

  useLayoutEffect(() => {
    const surface = workspace.current;
    if (!surface) return;
    const toolbar = surface.querySelector<HTMLElement>('.h1-toolbar')!;
    const layers = projectViewOrder.map(name => {
      const layer = surface.querySelector<HTMLElement>(`[data-workspace-view="${name}"]`)!;
      return { name, layer, contents: layer.querySelectorAll<HTMLElement>('[data-workspace-content]') };
    });
    const from = painted.current;
    const apply = (seconds: number) => {
      const mix = blendProjectViews(from.surface, view, ease(seconds / projectViewBlendSeconds));
      const content = projectViewContentOpacities(from.content, view, seconds);
      painted.current = { surface: mix, content };
      const opacity = projectViewOpacities(mix);
      for (const { name, layer, contents } of layers) {
        const alpha = String(opacity[name]);
        const visibility = mix[name] > 0 || name === view ? 'visible' : 'hidden';
        if (layer.style.opacity !== alpha) layer.style.opacity = alpha;
        if (layer.style.visibility !== visibility) layer.style.visibility = visibility;
        const contentAlpha = String(content[name]);
        for (const node of contents) {
          if (node.style.opacity !== contentAlpha) node.style.opacity = contentAlpha;
        }
      }
      // Limit color recalculation to the shared toolbar, outside the live terminal and logs.
      toolbar.style.setProperty('--terminal-view-share', String(mix.terminal));
    };
    if (reduced || from.surface[view] === 1) { apply(projectViewBlendSeconds); return; }

    apply(0);
    let lastTime = gsap.globalTimeline.time();
    let elapsed = 0;
    const tick = () => {
      const time = gsap.globalTimeline.time();
      const frameSeconds = time - lastTime;
      lastTime = time;
      if (frameSeconds <= 0) return;
      elapsed = advanceProjectViewTime(elapsed, frameSeconds);
      const complete = elapsed === projectViewBlendSeconds;
      apply(elapsed);
      if (complete) gsap.ticker.remove(tick);
    };
    gsap.ticker.add(tick);
    // Keep the painted mix when the user redirects an unfinished blend.
    return () => gsap.ticker.remove(tick);
  }, [workspace, view, reduced]);
}
