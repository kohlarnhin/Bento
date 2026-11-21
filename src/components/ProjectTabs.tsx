import { useCallback, useRef, useState } from 'react';
import { Plus, RotateCcw, Sparkle, X } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectView, TerminalAgent } from '../types';
import { closeTerminal, getTerminalSnapshot, restartTerminal, terminalKey, type TerminalStatus } from '../lib/terminalRegistry';
import { agentNames } from '../lib/terminalUsability';
import { SidebarMenu } from './SidebarMenu';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { SegmentedControl } from './ui/segmented-control';
import { MotionPresence } from './ui/motion-presence';
import { ActionTooltip } from './ui/tooltip';

type TerminalAction = 'restart' | 'close';
type Request = { action: TerminalAction; slot: number; sessionId: string };
const actionLabel = { restart: '重启', close: '关闭' };

function TabLabel({ label, agent, attention, completion }: { label: string; agent?: TerminalAgent | null; attention?: boolean; completion?: boolean }) {
  return <span className="workspace-tab-label" data-tab-label={label} data-terminal-agent-kind={agent?.kind}>
    <span>{agent && <Sparkle className="terminal-agent-sparkle" size={12} strokeWidth={1.5} aria-hidden="true" />}
      {label}{agent?.state === 'waiting' && <span className="terminal-agent-waiting-dot" role="img" aria-label="等你确认" />}</span>
    {!agent && (completion ? <span className="h1-status-dot terminal-tab-attention" aria-label="命令已结束" data-terminal-completion-dot />
      : attention && <span className="h1-status-dot terminal-tab-attention" data-tone="warning" aria-label="终端有新动静" />)}
  </span>;
}

interface Props {
  projectId: string;
  view: ProjectView;
  activeSlot: number;
  slots: number[];
  terminals: Record<string, TerminalStatus>;
  maxTerminals: number;
  onSelectView: (view: 'overview' | 'git') => void;
  onSelectTerminal: (slot: number) => void;
  onNewTerminal: () => Promise<void>;
  onTerminalClosed: (slot: number) => void;
}

export function ProjectTabs({ projectId, view, activeSlot, slots, terminals, maxTerminals,
  onSelectView, onSelectTerminal, onNewTerminal, onTerminalClosed }: Props) {
  const [menu, setMenu] = useState<{ slot: number; x: number; y: number }>();
  const [confirmation, setConfirmation] = useState<Request>();
  const [creating, setCreating] = useState(false);
  const [busy, setBusy] = useState(false);
  const executing = useRef(false);
  const closeMenu = useCallback(() => setMenu(undefined), []);
  const terminalView = view === 'terminal';
  const atLimit = slots.length >= maxTerminals;
  const createLabel = atLimit ? `已达上限（${maxTerminals} 个），可在设置中调整` : '新建终端 (⌘T)';

  async function perform({ action, slot, sessionId }: Request) {
    if (executing.current || getTerminalSnapshot()[terminalKey(projectId, slot)]?.sessionId !== sessionId) {
      setConfirmation(undefined);
      return;
    }
    executing.current = true;
    setBusy(true);
    try {
      if (action === 'restart') await restartTerminal(projectId, slot);
      else {
        await closeTerminal(projectId, slot);
        onTerminalClosed(slot);
      }
      setConfirmation(undefined);
    } catch (error) {
      toast.error(`${actionLabel[action]}终端失败：${error}`);
    } finally {
      executing.current = false;
      setBusy(false);
    }
  }

  function request(action: TerminalAction, slot: number) {
    const status = getTerminalSnapshot()[terminalKey(projectId, slot)];
    if (executing.current || !status?.sessionId || status.opening) return;
    const next = { action, slot, sessionId: status.sessionId };
    if (status.alive) setConfirmation(next);
    else void perform(next);
  }

  const menuStatus = menu && terminals[terminalKey(projectId, menu.slot)];
  const disabled = busy || !menuStatus?.sessionId || menuStatus.opening;
  const add = <ActionTooltip label={createLabel}>
    <span className="terminal-tab-add-wrap" title={createLabel} data-view-control aria-hidden={!terminalView || undefined}>
      <button type="button" aria-label={createLabel} aria-keyshortcuts="Meta+T" className="terminal-tab-add" data-terminal-tab-add
        disabled={!terminalView || atLimit || creating} onClick={() => {
          setCreating(true);
          void onNewTerminal().finally(() => setCreating(false));
        }}><Plus size={15} aria-hidden="true" /></button>
    </span>
  </ActionTooltip>;

  const agentCounts = { claude: 0, codex: 0 };
  const tabs = [
    { value: 'overview', label: <TabLabel label="概览" /> },
    ...slots.map(slot => {
      const status = terminals[terminalKey(projectId, slot)];
      const agent = status?.agent;
      const number = agent ? ++agentCounts[agent.kind] : 0;
      const slotName = `终端 ${slot}`;
      const label = agent ? agentNames[agent.kind] + (number > 1 ? ` ${number}` : '') : slotName;
      return {
        value: `terminal:${slot}`, title: agent && status?.title ? `${slotName} · ${status.title}` : status?.title || slotName,
        label: <TabLabel label={label} agent={agent} attention={status?.attention} completion={!!status?.completion} />,
        onContextMenu: (event: React.MouseEvent<HTMLButtonElement>) => {
          event.preventDefault();
          const bounds = event.currentTarget.getBoundingClientRect();
          setMenu({ slot, x: event.clientX || bounds.left, y: event.clientY || bounds.bottom });
        },
        accessory: <>{status?.sessionId && <button type="button" className="terminal-tab-close" title={`关闭终端 ${slot}`}
          aria-label={`关闭终端 ${slot}`} data-terminal-tab-close={slot} disabled={busy || status.opening}
          onClick={() => request('close', slot)}><X size={10} aria-hidden="true" /></button>}</>,
      };
    }),
    { value: 'git', label: <TabLabel label="Git" /> },
  ];

  return <>
    <div className="workspace-tab-controls">
      <SegmentedControl className="h1-workspace-tabs" label="项目视图" options={tabs}
        value={terminalView ? `terminal:${activeSlot}` : view} onChange={next => {
          if (next === 'overview' || next === 'git') onSelectView(next);
          else onSelectTerminal(Number(next.slice('terminal:'.length)));
        }} />
      {add}
    </div>
    <MotionPresence>{menu && <SidebarMenu x={menu.x} y={menu.y} label={`终端 ${menu.slot} 操作`} className={terminalView ? 'terminal-chrome' : undefined}
      items={[
        { label: '重启', icon: <RotateCcw size={14} />, disabled, onSelect: () => request('restart', menu.slot) },
        { label: '关闭', icon: <X size={14} />, disabled, onSelect: () => request('close', menu.slot) },
      ]} onClose={closeMenu} />}</MotionPresence>
    <MotionPresence>{confirmation && <Dialog open onOpenChange={open => { if (!open && !busy) setConfirmation(undefined); }}>
      <DialogContent hideClose={busy} data-terminal-tab-confirm>
        <DialogHeader>
          <DialogTitle>{actionLabel[confirmation.action]}终端 {confirmation.slot}</DialogTitle>
          <DialogDescription>此终端和其中正在运行的命令将结束。</DialogDescription>
        </DialogHeader>
        <div className="flex justify-end gap-2">
          <Button variant="outline" disabled={busy} onClick={() => setConfirmation(undefined)}>取消</Button>
          <Button variant="destructive" disabled={busy} onClick={() => { void perform(confirmation); }}>{actionLabel[confirmation.action]}</Button>
        </div>
      </DialogContent>
    </Dialog>}</MotionPresence>
  </>;
}
