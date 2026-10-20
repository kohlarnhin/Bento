import { useEffect, useRef, useSyncExternalStore } from 'react';
import { FileUp, Loader2, RotateCcw, TriangleAlert } from 'lucide-react';
import type { ProjectConfig } from '../types';
import { attachTerminal, fitTerminal, getTerminalSnapshot, restartTerminal, retryTerminal, subscribeTerminals } from '../lib/terminalRegistry';
import Button from './ui/button';

export function ProjectTerminal({ project }: { project: ProjectConfig }) {
  const host = useRef<HTMLDivElement>(null);
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const status = terminals[project.id];

  useEffect(() => {
    if (!host.current) return;
    const detach = attachTerminal(project.id, host.current);
    let timeout: ReturnType<typeof setTimeout>;
    const observer = new ResizeObserver(() => {
      clearTimeout(timeout);
      timeout = setTimeout(() => fitTerminal(project.id), 80);
    });
    observer.observe(host.current);
    return () => { clearTimeout(timeout); observer.disconnect(); detach(); };
  }, [project.id]);

  const exited = !!status?.sessionId && !status.alive && !status.opening;
  const overlay = status?.opening || (!status?.alive && status?.error) || exited;
  return (
    <section className="group/pane flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-terminal text-terminal-foreground" data-terminal-pane={project.id} aria-label={`${project.displayName || project.name} 终端`}>
      <div data-tauri-drag-region className="h-7 shrink-0" aria-hidden="true" />
      <div className="relative min-h-0 min-w-0 flex-1 p-3" data-terminal-area>
        <div ref={host} className="size-full min-h-0 min-w-0" />
        {overlay && <div className="absolute inset-0 flex items-center justify-center bg-terminal/60 p-6" data-terminal-state-overlay>
          <div className="flex max-w-md flex-col items-center gap-4 rounded-lg border border-terminal-border bg-terminal p-6 text-center shadow-xl">
            {status?.opening ? <><Loader2 className="size-5 animate-spin text-terminal-muted" /><p className="text-sm" role="status">正在打开终端…</p></> : <>
              {status?.error ? <TriangleAlert className="size-5 text-terminal-muted" /> : <RotateCcw className="size-5 text-terminal-muted" />}
              <p className="break-all text-sm" role={status?.error ? 'alert' : 'status'}>{status?.error || `终端已退出（状态码 ${status?.exitCode ?? '未知'}）`}</p>
              <Button variant="terminal" size="sm" onClick={() => { if (exited) void restartTerminal(project.id); else retryTerminal(project.id); }} data-terminal-restart>
                <RotateCcw data-icon="inline-start" />{exited ? '重新启动终端' : '重试'}
              </Button>
            </>}
          </div>
        </div>}
        <div className="pointer-events-none absolute inset-3 hidden flex-col items-center justify-center gap-4 rounded-lg border-2 border-dashed border-terminal-muted bg-terminal/95 group-data-[file-drag=true]/pane:flex" data-terminal-drop-overlay>
          <FileUp className="size-8 text-terminal-muted" /><p className="text-sm font-medium">松开以插入文件路径</p>
        </div>
      </div>
    </section>
  );
}
