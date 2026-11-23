import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { dashboardLog, dashboardServices, logTime } from '../lib/dashboard';
import { getTerminal, terminalKey, type TerminalStatus } from '../lib/terminalRegistry';
import type { AgentRecoveries, OpenAgent } from '../lib/agentSessions';
import { agentNames, agentNeedsAttention, agentStateLabels, terminalDisplayNumber } from '../lib/terminalUsability';
import { useProjectLogLines } from '../lib/useProjectLogs';
import type { ProjectConfig, Service, Snippet, SnippetChange } from '../types';
import { ProjectTerminal, type ProjectTerminalControls, type TerminalDimensions } from './ProjectTerminal';
import { AgentElapsedTime } from './AgentElapsedTime';
import { TerminalSnippets } from './TerminalSnippets';
import { LogText } from './LogText';
import { Select } from './ui/select';

export interface TerminalWorkspaceControls extends ProjectTerminalControls {
  toggleSplit: () => void;
  focusInput: () => void;
}

interface TerminalContext { shell: string | null; cwd: string | null }

function useTerminalContext(projectId: string, slot: number, session: TerminalStatus | undefined, interactive: boolean) {
  const [context, setContext] = useState<{ sessionId: string; value: TerminalContext }>();
  const sessionId = session?.sessionId;
  useEffect(() => {
    if (!interactive || !sessionId || !session?.alive) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const update = async () => {
      const value = await invoke<TerminalContext | null>('terminal_context', { projectId, slot, sessionId }).catch(() => null);
      if (disposed) return;
      if (value) setContext(previous => previous?.sessionId === sessionId &&
        previous.value.shell === value.shell && previous.value.cwd === value.cwd ? previous : { sessionId, value });
      timer = setTimeout(update, 1500);
    };
    void update();
    return () => { disposed = true; clearTimeout(timer); };
  }, [projectId, slot, sessionId, session?.alive, interactive]);
  return context?.sessionId === sessionId ? context?.value : undefined;
}

function ServiceLogPane({ projectId, services, serviceId }: { projectId: string; services: Service[]; serviceId?: string }) {
  const logs = useProjectLogLines(projectId);
  const lines = useMemo(() => logs.map(entry => dashboardLog(entry, services))
    .filter(line => serviceId === undefined || line.service?.id === serviceId), [logs, services, serviceId]);
  const area = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  useLayoutEffect(() => {
    if (area.current && follow.current) area.current.scrollTop = area.current.scrollHeight;
  }, [lines.at(-1)?.id]);
  return <div ref={area} className="terminal-service-log" data-terminal-service-log tabIndex={0} aria-label="服务日志"
    onScroll={event => {
      const log = event.currentTarget;
      follow.current = log.scrollHeight - log.scrollTop - log.clientHeight < 26;
    }}>
    {lines.map(line => <div key={line.id} className="terminal-log-line" data-severity={line.severity}>
      <time>{logTime(line.timestamp)}</time><span>{serviceId === undefined && line.service && `${line.service.name} `}<LogText text={line.text} /></span>
    </div>)}
    {!lines.length && <p className="terminal-log-empty">暂无日志</p>}
  </div>;
}

const MIN_SPLIT = 25;
const MAX_SPLIT = 75;

export const TerminalWorkspace = forwardRef<TerminalWorkspaceControls, {
  project: ProjectConfig;
  activeSlot: number;
  slots: number[];
  terminals: Record<string, TerminalStatus>;
  agentRecoveries: AgentRecoveries;
  onOpenAgent: OpenAgent;
  onDismissAgentRecovery: (sessionId: string) => void;
  focused: boolean;
  interactive: boolean;
  ready: boolean;
  showQuickCommands: boolean;
  snippets: Snippet[];
  onRunSnippet: (command: string) => void;
  onChangeSnippet: (change: SnippetChange) => Promise<void>;
  onReady: (slot: number) => void;
  onFocusedSlot: (projectId: string, slot: number, left: number, right?: number) => void;
  onJumpToAttention: () => void;
}>(function TerminalWorkspace({ project, activeSlot, slots, terminals, focused, interactive, ready,
  agentRecoveries, onOpenAgent, onDismissAgentRecovery, showQuickCommands, snippets, onRunSnippet, onChangeSnippet, onReady, onFocusedSlot, onJumpToAttention }, ref) {
  const [split, setSplit] = useState(false);
  const [source, setSource] = useState<string>();
  const [splitWidth, setSplitWidth] = useState(50);
  const [inputPane, setInputPane] = useState<'left' | 'right'>('left');
  const [dimensions, setDimensions] = useState<Record<number, TerminalDimensions>>({});
  const panes = useRef<HTMLDivElement>(null);
  const leftControls = useRef<ProjectTerminalControls>(null);
  const rightControls = useRef<ProjectTerminalControls>(null);
  const dragFrame = useRef<DOMRect>();
  const services = useMemo(() => dashboardServices(project), [project.port, project.services]);
  const logOptions = [{ value: 'logs', label: '合并日志' }, ...services.map(service => ({ value: `log:${service.id}`, label: `${service.name} · 日志` }))];
  const peers = slots.filter(slot => slot !== activeSlot);
  const options = [...peers.map(slot => ({ value: `terminal:${slot}`, label: `终端 ${terminalDisplayNumber(slots, slot)}` })), ...logOptions];
  const selected = options.some(option => option.value === source) ? source! : logOptions[services.length > 1 ? 1 : 0].value;
  const rightSlot = split ? peers.find(slot => selected === `terminal:${slot}`) : undefined;
  const activeNumber = terminalDisplayNumber(slots, activeSlot)!;
  const rightService = selected.startsWith('log:') ? selected.slice('log:'.length) : undefined;
  const inputSlot = (inputPane === 'right' ? rightSlot : undefined) ?? activeSlot;
  const currentInputSlot = useRef(inputSlot);
  currentInputSlot.current = inputSlot;
  const terminalStatus = terminals[terminalKey(project.id, inputSlot)];
  const agent = terminalStatus?.agent;
  const otherPending = agent ? Object.values(terminals).filter(terminal => terminal !== terminalStatus && agentNeedsAttention(terminal.agent)).length : 0;
  const context = useTerminalContext(project.id, inputSlot, terminalStatus, interactive && !agent);
  const size = dimensions[inputSlot];
  const shell = context?.shell ?? 'shell';
  const cwd = context?.cwd ?? project.path;

  const resize = useCallback((slot: number, next: TerminalDimensions) => {
    setDimensions(previous => previous[slot]?.rows === next.rows && previous[slot]?.cols === next.cols
      ? previous : { ...previous, [slot]: next });
  }, []);
  useEffect(() => { onFocusedSlot(project.id, inputSlot, activeSlot, rightSlot); },
    [project.id, inputSlot, activeSlot, rightSlot, onFocusedSlot]);
  useLayoutEffect(() => setInputPane('left'), [activeSlot]);
  useEffect(() => {
    if (source?.startsWith('terminal:') && !slots.includes(Number(source.slice('terminal:'.length)))) setSource(undefined);
  }, [source, slots]);
  useLayoutEffect(() => {
    if (!interactive || !ready) return;
    const frame = requestAnimationFrame(() => getTerminal(project.id, currentInputSlot.current)?.focus());
    return () => cancelAnimationFrame(frame);
  }, [interactive, ready, project.id, activeSlot]);

  const inputControls = inputSlot === activeSlot ? leftControls : rightControls;
  useImperativeHandle(ref, () => ({
    openFind: () => inputControls.current?.openFind(),
    closeFind: () => inputControls.current?.closeFind(),
    isFindOpen: () => inputControls.current?.isFindOpen() ?? false,
    navigateFind: previous => inputControls.current?.navigateFind(previous),
    focusInput: () => getTerminal(project.id, inputSlot)?.focus(),
    toggleSplit: () => {
      if (split && inputSlot !== activeSlot) {
        setInputPane('left');
        getTerminal(project.id, activeSlot)?.focus();
      }
      setSplit(!split);
    },
  }));

  const setSplitPosition = (percent: number) => setSplitWidth(Math.round(Math.max(MIN_SPLIT, Math.min(MAX_SPLIT, percent))));
  const focusLeft = () => setInputPane('left');
  const focusRight = () => setInputPane('right');
  const sourceChanged = (next: string) => {
    setSource(next);
    setInputPane('left');
  };

  return <div className="terminal-workspace" data-terminal-workspace={focused ? 'focus' : 'immersive'} data-workspace-content>
    <div ref={panes} className="terminal-panes" data-split={split || undefined}
      style={split ? { gridTemplateColumns: `minmax(0, ${splitWidth}fr) 1px minmax(0, ${100 - splitWidth}fr)` } : undefined}>
      <div className="terminal-pane-frame">
        {split && <div className="terminal-pane-heading">终端 {activeNumber}{inputSlot === activeSlot && context?.shell && ` · ${shell}`}</div>}
        <ProjectTerminal project={project} activeSlot={activeSlot} terminalNumber={activeNumber} ref={leftControls} interactive={interactive}
          agentRecoveries={agentRecoveries} onOpenAgent={onOpenAgent} onDismissAgentRecovery={onDismissAgentRecovery}
          onReady={onReady} onFocus={focusLeft} onResize={resize} />
      </div>
      {split && <>
        <div className="terminal-splitter" role="separator" aria-orientation="vertical" aria-label="分屏宽度" tabIndex={0}
          aria-valuemin={MIN_SPLIT} aria-valuemax={MAX_SPLIT} aria-valuenow={splitWidth} data-terminal-splitter
          onPointerDown={event => {
            if (event.button !== 0 || !panes.current) return;
            event.preventDefault();
            dragFrame.current = panes.current.getBoundingClientRect();
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={event => {
            const frame = dragFrame.current;
            if (frame && event.currentTarget.hasPointerCapture(event.pointerId)) setSplitPosition(100 * (event.clientX - frame.left) / frame.width);
          }}
          onLostPointerCapture={() => { dragFrame.current = undefined; }}
          onKeyDown={event => {
            if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
            event.preventDefault();
            setSplitPosition(event.key === 'Home' ? MIN_SPLIT : event.key === 'End' ? MAX_SPLIT : splitWidth + (event.key === 'ArrowLeft' ? -5 : 5));
          }} />
        <div className="terminal-pane-frame" data-terminal-secondary>
          <div className="terminal-pane-heading">
            <Select id="terminal-split-source" label="右侧窗格内容" value={selected} options={options}
              onValueChange={sourceChanged} className="terminal-source-select" popupClassName="terminal-chrome" />
          </div>
          {rightSlot === undefined ? <ServiceLogPane key={selected} projectId={project.id} services={services} serviceId={rightService} /> :
            <ProjectTerminal project={project} activeSlot={rightSlot} terminalNumber={terminalDisplayNumber(slots, rightSlot)!}
              ref={rightControls} interactive={interactive} onFocus={focusRight} onResize={resize}
              agentRecoveries={agentRecoveries} onOpenAgent={onOpenAgent} onDismissAgentRecovery={onDismissAgentRecovery} />}
        </div>
      </>}
    </div>
    {showQuickCommands && <TerminalSnippets projectId={project.id} entries={snippets} onRun={onRunSnippet} onChange={onChangeSnippet} />}
    <footer className="terminal-status" data-terminal-status data-terminal-agent={agent?.state}>
      {agent ? <>
        <span className="agent-status-chip" data-agent-state={agent.state}>
          {agent.state === 'thinking' && <span className="agent-thinking-ring" aria-hidden="true" />}
          {agentStateLabels[agent.state]}
        </span>
        <span className="truncate">{agent.kind === 'claude' ? 'Claude Code' : agentNames[agent.kind]}
          {agent.roundStartedAt != null && <> · 本轮 <AgentElapsedTime startAt={agent.roundStartedAt}
            endAt={agent.state === 'done' ? agent.stateStartedAt : undefined} paused={!interactive} /></>}
        </span>
        {otherPending > 0 && <button type="button" className="terminal-status-next" aria-keyshortcuts="Meta+Shift+U"
          onClick={onJumpToAttention}>还有 {otherPending} 个会话在等 · ⌘⇧U 下一个</button>}
      </> : <>
        <span data-terminal-status-shell>{shell}</span>
        <span className="truncate" title={cwd} data-terminal-status-cwd>{cwd}</span>
        <span className="shrink-0" data-terminal-status-size>{size ? `${size.cols}×${size.rows}` : '—'}</span>
        <span className="terminal-status-shortcuts">⌘D 分屏 · ⌘⇧↩ {focused ? '退出专注' : '专注'}</span>
      </>}
    </footer>
  </div>;
});
