import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { FileUp, Loader2, Plus, RotateCcw, TriangleAlert, X } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig } from '../types';
import { attachTerminal, closeTerminal, fitTerminal, getProjectTerminalSlots, getTerminalSnapshot, restartTerminal, retryTerminal, subscribeTerminals, terminalKey, type TerminalStatus } from '../lib/terminalRegistry';
import { cn } from '../lib/utils';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';

function TerminalTab({ projectId, slot, active, status, onSelect }: {
  projectId: string; slot: number; active: boolean; status?: TerminalStatus; onSelect: () => void;
}) {
  const [confirmUntil, setConfirmUntil] = useState(0);
  const [closing, setClosing] = useState(false);
  const pending = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    setConfirmUntil(0);
    return () => clearTimeout(timer.current);
  }, [status?.sessionId]);
  const confirming = !!status?.alive && confirmUntil > 0;
  const closeLabel = confirming ? '再次点击确认关闭' : `关闭终端 ${slot}`;
  return <div className={cn('group/tab no-drag flex h-7 shrink-0 items-center rounded-md border border-transparent', active ? 'border-terminal-border bg-terminal-border/60' : 'hover:bg-terminal-border/40')}>
    <button role="tab" aria-selected={active} aria-label={`终端 ${slot}`} data-terminal-tab={slot}
      className={cn('no-drag flex h-full items-center gap-2 rounded-md px-2.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-terminal-muted', !active && 'text-terminal-muted')}
      onClick={onSelect}>
      <span className={cn('size-1.5 rounded-full', status?.attention ? 'bg-attention' : status?.alive ? 'bg-success' : 'bg-terminal-muted/50')}
        aria-label={status?.attention ? '终端有新动静' : status?.alive ? '终端运行中' : '终端未运行'} data-terminal-tab-attention={status?.attention ? slot : undefined} />
      终端 {slot}
    </button>
    {slot > 1 && <ActionTooltip label={closeLabel} side="bottom">
      <Button variant={confirming ? 'ghost-destructive' : 'terminal'} size="icon-sm" className={cn('no-drag mr-0.5 size-5 border-0 bg-transparent opacity-0 group-hover/tab:opacity-100 group-focus-within/tab:opacity-100 [&_svg]:size-3', (active || confirming) && 'opacity-100')}
        aria-label={closeLabel} title={closeLabel} data-terminal-tab-close={slot} data-confirming={confirming} disabled={closing || status?.opening}
        onClick={() => {
          if (pending.current) return;
          if (status?.alive && Date.now() >= confirmUntil) {
            setConfirmUntil(Date.now() + 3000);
            clearTimeout(timer.current);
            timer.current = setTimeout(() => setConfirmUntil(0), 3000);
            return;
          }
          clearTimeout(timer.current);
          setConfirmUntil(0);
          pending.current = true;
          setClosing(true);
          void closeTerminal(projectId, slot).catch(error => toast.error(`关闭终端失败：${error}`)).finally(() => {
            pending.current = false;
            setClosing(false);
          });
        }}><X /></Button>
    </ActionTooltip>}
  </div>;
}

export function ProjectTerminal({ project, activeSlot, maxPerProject, onSelectSlot, onCreate }: {
  project: ProjectConfig; activeSlot: number; maxPerProject: number;
  onSelectSlot: (slot: number) => void; onCreate: () => Promise<void>;
}) {
  const host = useRef<HTMLDivElement>(null);
  const terminals = useSyncExternalStore(subscribeTerminals, getTerminalSnapshot);
  const status = terminals[terminalKey(project.id, activeSlot)];
  const slots = getProjectTerminalSlots(project.id, terminals);
  const [creating, setCreating] = useState(false);
  const atLimit = slots.length >= maxPerProject;
  const newLabel = atLimit ? `已达上限（${maxPerProject} 个），可在设置中调整` : '新建终端 (⌘T)';

  useEffect(() => {
    if (!host.current || (activeSlot > 1 && !status?.sessionId)) return;
    const detach = attachTerminal(project.id, activeSlot, host.current);
    let timeout: ReturnType<typeof setTimeout>;
    const observer = new ResizeObserver(() => {
      clearTimeout(timeout);
      timeout = setTimeout(() => fitTerminal(project.id, activeSlot), 80);
    });
    observer.observe(host.current);
    return () => { clearTimeout(timeout); observer.disconnect(); detach(); };
  }, [project.id, activeSlot, status?.sessionId]);

  const exited = !!status?.sessionId && !status.alive && !status.opening;
  const overlay = status?.opening || (!status?.alive && status?.error) || exited;
  return (
    <section className="group/pane flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-terminal text-terminal-foreground" data-terminal-pane={project.id} data-terminal-active-slot={activeSlot} aria-label={`${project.displayName || project.name} 终端`}>
      <div data-tauri-drag-region className="flex h-9 shrink-0 items-center gap-1 border-b border-terminal-border/60 px-2">
        <div className="no-drag flex min-w-0 items-center gap-1 overflow-x-auto" role="tablist" aria-label="项目终端">
          {slots.map(slot => <TerminalTab key={slot} projectId={project.id} slot={slot} active={slot === activeSlot} status={terminals[terminalKey(project.id, slot)]} onSelect={() => onSelectSlot(slot)} />)}
        </div>
        <ActionTooltip label={newLabel} side="bottom">
          <span className="no-drag inline-flex shrink-0" title={newLabel}>
            <Button variant="terminal" size="icon-sm" className="no-drag border-0 bg-transparent text-terminal-muted" aria-label="新建终端" data-terminal-new disabled={atLimit || creating}
              onClick={() => {
                setCreating(true);
                void onCreate().finally(() => setCreating(false));
              }}><Plus /></Button>
          </span>
        </ActionTooltip>
        <div data-tauri-drag-region className="h-full min-w-3 flex-1" />
      </div>
      <div className="relative min-h-0 min-w-0 flex-1 p-3" data-terminal-area>
        <div ref={host} className="size-full min-h-0 min-w-0" />
        {overlay && <div className="absolute inset-0 flex items-center justify-center bg-terminal/60 p-6" data-terminal-state-overlay>
          <div className="flex max-w-md flex-col items-center gap-4 rounded-lg border border-terminal-border bg-terminal p-6 text-center shadow-xl">
            {status?.opening ? <><Loader2 className="size-5 animate-spin text-terminal-muted" /><p className="text-sm" role="status">正在打开终端…</p></> : <>
              {status?.error ? <TriangleAlert className="size-5 text-terminal-muted" /> : <RotateCcw className="size-5 text-terminal-muted" />}
              <p className="break-all text-sm" role={status?.error ? 'alert' : 'status'}>{status?.error || `终端已退出（状态码 ${status?.exitCode ?? '未知'}）`}</p>
              <Button variant="terminal" size="sm" onClick={() => { if (exited) void restartTerminal(project.id, activeSlot); else retryTerminal(project.id, activeSlot); }} data-terminal-restart>
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
