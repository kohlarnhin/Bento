import { Sparkle, SquareTerminal } from 'lucide-react';
import { agentArgv, agentSessionDate, type AgentArgv, type AgentHistorySession, type AgentMenuData } from '../lib/agentSessions';
import { durationText } from '../lib/dashboard';
import { agentNames } from '../lib/terminalUsability';

export function AgentSessionMenuItems({ data, atLimit, onTerminal, onAgent }: {
  data?: AgentMenuData;
  atLimit: boolean;
  onTerminal: () => void;
  onAgent: (argv: AgentArgv) => void;
}) {
  const sessions = data?.sessions.slice(0, 8);
  return <>
    <button type="button" role="menuitem" disabled={atLimit} onClick={onTerminal} aria-keyshortcuts="Meta+T">
      <SquareTerminal aria-hidden="true" /><span className="flex-1">新终端</span><kbd>⌘T</kbd>
    </button>
    {!data && <div className="terminal-agent-menu-heading" role="status">正在读取 AI 会话…</div>}
    {data?.tools.map(tool => <button key={tool} type="button" role="menuitem" disabled={atLimit} onClick={() => onAgent(agentArgv(tool))}>
      <Sparkle aria-hidden="true" />新 {agentNames[tool]} 会话
    </button>)}
    {sessions && sessions.length > 0 && <>
      <div role="separator" className="h1-menu-divider" />
      <div role="group" aria-label="继续之前的 AI 会话 · 这个项目">
        <div className="terminal-agent-menu-heading" aria-hidden="true">继续之前的 AI 会话 · 这个项目</div>
        <div className="terminal-agent-session-list">
          {sessions.map(session => <button key={`${session.tool}:${session.id}`} type="button" role="menuitem"
            className="terminal-agent-session-option" title={session.title} data-agent-session-option
            onClick={() => onAgent(agentArgv(session.tool, session.id))}>
            <Sparkle aria-hidden="true" />
            <span className="min-w-0 flex-1">
              <span className="block truncate">{session.title}</span>
              <span className="terminal-agent-session-meta block">{agentNames[session.tool]} · {agentSessionDate(session.lastAt)}</span>
            </span>
            <span className="terminal-agent-session-meta whitespace-nowrap">
              {durationText(session.durationMs)}{session.fileCount != null && ` · ${session.fileCount} 个文件`}
            </span>
          </button>)}
        </div>
      </div>
    </>}
  </>;
}

export function AgentRecoveryCard({ session, busy, onContinue, onIgnore }: {
  session: AgentHistorySession;
  busy: boolean;
  onContinue: (argv: AgentArgv) => void;
  onIgnore: () => void;
}) {
  const heading = `这里上次是 ${agentNames[session.tool]} 会话「${session.title}」`;
  return <div className="terminal-agent-recovery" data-agent-recovery-card>
    <Sparkle size={15} aria-hidden="true" />
    <div className="terminal-agent-recovery-copy">
      <div className="truncate" title={heading}>{heading}</div>
      <p>重启后终端只恢复了输出 · 继续会带上完整对话上下文</p>
    </div>
    <div className="terminal-agent-recovery-actions">
      <button type="button" className="h1-button h1-button-primary" data-agent-recovery-continue disabled={busy}
        onClick={() => onContinue(agentArgv(session.tool, session.id))}>继续会话 <span aria-hidden="true">↵</span></button>
      <button type="button" className="h1-button" data-agent-recovery-ignore disabled={busy} onClick={onIgnore}>忽略</button>
    </div>
  </div>;
}
