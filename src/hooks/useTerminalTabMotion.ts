import { useLayoutEffect, useRef, useState, type RefObject } from 'react';
import gsap from 'gsap';
import { useReducedMotion } from '../lib/motion';
import { animateTerminalTab, settledTerminalTab } from '../lib/motionMath';

export function useTerminalTabMotion(panel: RefObject<HTMLElement>, terminalId: string | undefined, terminal: boolean, focusInput: () => void) {
  const reduced = useReducedMotion();
  const painted = useRef(settledTerminalTab(terminal));
  const [retainedId, setRetainedId] = useState(terminal ? terminalId : undefined);
  const showTerminal = terminal || (!reduced && terminalId !== undefined && retainedId === terminalId);

  useLayoutEffect(() => {
    const main = panel.current;
    if (!main) return;
    const workspace = main.querySelector<HTMLElement>('.terminal-workspace');
    if (workspace) {
      const restoreFocus = terminal && workspace.inert;
      workspace.inert = !terminal;
      if (restoreFocus) focusInput();
    }

    const finish = () => {
      painted.current = settledTerminalTab(terminal);
      main.toggleAttribute('data-terminal-palette', terminal);
      main.style.removeProperty('--terminal-backdrop-opacity');
      main.style.removeProperty('--terminal-content-opacity');
      main.style.removeProperty('--terminal-content-translate');
      setRetainedId(terminal ? terminalId : undefined);
    };
    if (reduced || !showTerminal) { finish(); return; }
    if (terminal) setRetainedId(terminalId);

    const paint = { ...painted.current };
    const apply = () => {
      // Copy the painted values so reverting an interrupted GSAP context cannot rewind them.
      painted.current = { backdrop: paint.backdrop, content: paint.content, y: paint.y };
      main.style.setProperty('--terminal-backdrop-opacity', String(paint.backdrop));
      main.style.setProperty('--terminal-content-opacity', String(paint.content));
      main.style.setProperty('--terminal-content-translate', '0 ' + paint.y + 'px');
      main.toggleAttribute('data-terminal-palette', paint.backdrop === 1);
    };
    const scope = gsap.context(() => {
      const timeline = gsap.timeline({ paused: true, onUpdate: apply, onComplete: finish });
      animateTerminalTab(timeline, paint, terminal);
      if (timeline.duration()) { apply(); timeline.play(); }
      else finish();
    }, main);
    return () => scope.revert();
  }, [panel, terminalId, terminal, reduced, showTerminal, focusInput]);

  return showTerminal;
}
