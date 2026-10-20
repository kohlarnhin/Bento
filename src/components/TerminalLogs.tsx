import { useEffect, useRef, useState } from 'react';
import { Trash2, Copy, Check, ArrowDown, Terminal } from 'lucide-react';
import { toast } from 'sonner';
import type { LogEntry, ProjectConfig } from '../types';
import { TERMINAL_THEME } from '../lib/terminalAppearance';
import Button from './ui/button';
import { Badge } from './ui/badge';
import { ActionTooltip } from './ui/tooltip';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from './ui/dialog';

interface TerminalLogsModalProps {
  project: ProjectConfig;
  logs: LogEntry[];
  onClose: () => void;
  onClear: () => void;
}

function cleanAnsi(text: string): string {
  return text.replace(/\x1B\[[0-9;]*[a-zA-Z]/g, '');
}

export function TerminalLogsModal({ project, logs, onClose, onClear }: TerminalLogsModalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (autoScroll && containerRef.current) containerRef.current.scrollTop = containerRef.current.scrollHeight;
  }, [logs, autoScroll]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(logs.map((log) => `[${log.stream}] ${cleanAnsi(log.text)}`).join('\n'));
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
      toast.success('日志已复制');
    } catch (error) {
      toast.error(`复制日志失败：${error}`);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="flex h-[640px] max-h-[85vh] max-w-4xl flex-col gap-0 overflow-hidden border-terminal-border bg-terminal p-0 text-terminal-foreground" closeClassName="text-terminal-muted" data-terminal-logs>
        <DialogHeader className="shrink-0 px-6 pb-4 pt-6 pr-12">
          <DialogTitle>{project.displayName || project.name}</DialogTitle>
          <DialogDescription className="text-terminal-muted">项目启动日志 · 仅保留本次运行的输出</DialogDescription>
        </DialogHeader>
        <div className="flex shrink-0 items-center gap-2 border-t border-terminal-border px-5 py-2">
          <Badge variant="outline" className="border-terminal-border text-terminal-muted">{logs.length} 行</Badge>
          <div className="ml-auto flex items-center gap-1">
            {!autoScroll && <Button size="sm" variant="terminal" onClick={() => setAutoScroll(true)}><ArrowDown data-icon="inline-start" />到底部</Button>}
            <ActionTooltip label={copied ? '已复制' : '复制全部日志'}><Button size="icon-sm" variant="terminal" aria-label="复制全部日志" onClick={handleCopy}>{copied ? <Check /> : <Copy />}</Button></ActionTooltip>
            <ActionTooltip label="清空日志"><Button size="icon-sm" variant="terminal" aria-label="清空日志" onClick={onClear}><Trash2 /></Button></ActionTooltip>
          </div>
        </div>
        <div ref={containerRef} onScroll={() => {
          if (!containerRef.current) return;
          const { scrollTop, scrollHeight, clientHeight } = containerRef.current;
          setAutoScroll(scrollHeight - scrollTop - clientHeight < 40);
        }} className="min-h-0 flex-1 overflow-y-auto bg-terminal p-5 font-mono text-xs leading-relaxed text-terminal-foreground select-text">
          {logs.length === 0 ? <div className="flex h-full flex-col items-center justify-center gap-3 text-terminal-muted select-none"><Terminal className="size-7" /><span>暂无启动日志</span></div> : logs.map((log) => (
            <div key={log.id} className="break-all" style={{ color: log.stream === 'stderr' ? TERMINAL_THEME.red : log.stream === 'system' ? TERMINAL_THEME.blue : undefined }}>
              <span className="mr-3 text-terminal-muted select-none">{new Date(log.timestamp).toLocaleTimeString()}</span>{cleanAnsi(log.text)}
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
