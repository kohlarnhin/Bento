import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';
import { FileUp, TriangleAlert } from 'lucide-react';
import type { ProjectConfig } from '../types';
import { attachTerminal, fitTerminal, getTerminal, getTerminalSearch, getTerminalSnapshot, retryTerminal, subscribeTerminals, terminalKey } from '../lib/terminalRegistry';
import { TerminalFindBar, type TerminalFindControls } from './TerminalFindBar';
import Button from './ui/button';

export interface TerminalDimensions { cols: number; rows: number }

export interface ProjectTerminalControls {
  openFind: () => void;
  closeFind: () => void;
  isFindOpen: () => boolean;
  navigateFind: (previous: boolean) => void;
}

export const ProjectTerminal = forwardRef<ProjectTerminalControls, {
  project: ProjectConfig;
  activeSlot: number;
  terminalNumber: number;
  interactive: boolean;
  onReady?: (slot: number) => void;
  onFocus: () => void;
  onResize: (slot: number, size: TerminalDimensions) => void;
}>(function ProjectTerminal({ project, activeSlot, terminalNumber, interactive, onReady, onFocus, onResize }, ref) {
  const host = useRef<HTMLDivElement>(null);
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const status = terminals[terminalKey(project.id, activeSlot)];
  const sessionId = status?.sessionId;
  const canAttach = Boolean(sessionId);
  const find = useRef<TerminalFindControls>(null);
  const [findRequest, setFindRequest] = useState<{ key: string; version: number }>();
  const findKey = `${terminalKey(project.id, activeSlot)}:${sessionId}`;
  const findOpen = findRequest?.key === findKey;
  const closeFind = () => {
    getTerminalSearch(project.id, activeSlot)?.clearDecorations();
    setFindRequest(undefined);
    getTerminal(project.id, activeSlot)?.focus();
  };
  useImperativeHandle(ref, () => ({
    openFind: () => setFindRequest(previous => ({ key: findKey, version: (previous?.version ?? 0) + 1 })),
    closeFind,
    isFindOpen: () => findOpen,
    navigateFind: previous => find.current?.navigate(previous),
  }));
  // A slot change invalidates the bar even if the user later returns to it.
  useEffect(() => { setFindRequest(undefined); }, [findKey]);
  useEffect(() => {
    if (!interactive && findOpen) {
      getTerminalSearch(project.id, activeSlot)?.clearDecorations();
      setFindRequest(undefined);
    }
  }, [interactive, findOpen, project.id, activeSlot]);

  useLayoutEffect(() => {
    const container = host.current;
    if (!container || !canAttach) return;
    const detach = attachTerminal(project.id, activeSlot, container, onReady && (() => onReady(activeSlot)));
    const terminal = getTerminal(project.id, activeSlot);
    const sizeSubscription = terminal?.onResize(size => onResize(activeSlot, size));
    if (terminal) onResize(activeSlot, { cols: terminal.cols, rows: terminal.rows });
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      if (container.closest('[data-terminal-layout-motion]')) return;
      timeout ??= setTimeout(() => {
        timeout = undefined;
        fitTerminal(project.id, activeSlot);
      }, 80);
    });
    observer.observe(container);
    return () => {
      clearTimeout(timeout);
      observer.disconnect();
      sizeSubscription?.dispose();
      detach();
    };
    // Session hydration and restart reuse the slot's renderer. Keep it attached through both.
  }, [project.id, activeSlot, canAttach, onReady, onResize]);

  return (
    <section className="group/pane relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden" data-terminal-pane={project.id} data-terminal-active-slot={activeSlot}
      aria-label={`${project.displayName || project.name} 终端 ${terminalNumber}`} onFocusCapture={onFocus}>
      <div className="relative min-h-0 min-w-0 flex-1" data-terminal-area>
        <div ref={host} className="size-full min-h-0 min-w-0" />
        {findOpen && getTerminalSearch(project.id, activeSlot) && <TerminalFindBar key={findKey}
          ref={find} search={getTerminalSearch(project.id, activeSlot)!} terminal={getTerminal(project.id, activeSlot)!}
          focusRequest={findRequest.version} onClose={closeFind} />}
        {status?.error && <div className="absolute inset-0 flex items-center justify-center bg-terminal/60 p-6" data-terminal-state-overlay>
          <div className="flex max-w-md flex-col items-center gap-4 rounded-lg border border-terminal-border bg-terminal p-6 text-center shadow-xl">
            <TriangleAlert className="size-5 text-terminal-muted" />
            <p className="break-all text-sm" role="alert">{status.error}</p>
            <Button variant="terminal" size="sm" onClick={() => retryTerminal(project.id, activeSlot)} data-terminal-restart>重试</Button>
          </div>
        </div>}
        <div className="pointer-events-none absolute inset-3 hidden flex-col items-center justify-center gap-4 rounded-lg border-2 border-dashed border-terminal-muted bg-terminal/95 group-data-[file-drag=true]/pane:flex" data-terminal-drop-overlay>
          <FileUp className="size-8 text-terminal-muted" /><p className="text-sm font-medium">松开以插入文件路径</p>
        </div>
      </div>
    </section>
  );
});
