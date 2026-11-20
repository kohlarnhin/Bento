import { useLayoutEffect } from 'react';
import gsap from 'gsap';
import { useReducedMotion } from '../lib/motion';

export function useButtonPressMotion() {
  const reduced = useReducedMotion();
  useLayoutEffect(() => {
    if (reduced) return;
    const pressed = new Map<number, HTMLElement>();
    const animations = new Map<HTMLElement, gsap.core.Tween>();
    const animate = (button: HTMLElement, release: boolean) => {
      animations.get(button)?.kill();
      if (!button.style.getPropertyValue('--press-scale')) gsap.set(button, { '--press-scale': 1 });
      const animation = gsap.to(button, {
        '--press-scale': release ? 1 : .97, duration: release ? .16 : .08, ease: 'power3.out',
        onComplete: () => {
          if (release) { button.style.removeProperty('--press-scale'); animations.delete(button); }
        },
      });
      animations.set(button, animation);
    };
    const down = (event: PointerEvent) => {
      if (event.button !== 0 || !(event.target instanceof Element)) return;
      const button = event.target.closest<HTMLElement>('button, [role="button"]');
      if (!button || button.matches(':disabled, [aria-disabled="true"], .sidebar-project-row') || button.closest('[inert]')) return;
      pressed.set(event.pointerId, button);
      animate(button, false);
    };
    const up = (event: PointerEvent) => {
      const button = pressed.get(event.pointerId);
      if (!button) return;
      pressed.delete(event.pointerId);
      animate(button, true);
    };
    const releaseAll = () => { pressed.forEach(button => animate(button, true)); pressed.clear(); };
    document.addEventListener('pointerdown', down, true);
    document.addEventListener('pointerup', up, true);
    document.addEventListener('pointercancel', up, true);
    window.addEventListener('blur', releaseAll);
    return () => {
      document.removeEventListener('pointerdown', down, true);
      document.removeEventListener('pointerup', up, true);
      document.removeEventListener('pointercancel', up, true);
      window.removeEventListener('blur', releaseAll);
      animations.forEach((animation, button) => {
        animation.kill();
        button.style.removeProperty('--press-scale');
      });
    };
  }, [reduced]);
}
