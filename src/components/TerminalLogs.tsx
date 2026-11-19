import { useLayoutEffect, useRef, useState } from 'react';
import { Trash2, Copy, Check, ArrowDown, Terminal } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig } from '../types';
import { TERMINAL_THEME } from '../lib/terminalAppearance';
import { cleanTerminalText } from '../lib/dashboard';
import { useProjectLogLines } from '../lib/useProjectLogs';
import Button from './ui/button';
import { Badge } from './ui/badge';
import { ActionTooltip } from './ui/tooltip';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from './ui/dialog';

interface TerminalLogsModalProps {
  project: ProjectConfig;
  onClose: () => void;
  onClear: () => void;
  focusEntry?: string;
}

export function TerminalLogsModal({ project, onClose, onClear, focusEntry }: TerminalLogsModalProps) {
  const logs = useProjectLogLines(project.id);
  const containerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(!focusEntry);
  const [copied, setCopied] = useState(false);
  useLayoutEffect(() => {
    if (autoScroll && containerRef.current) containerRef.current.scrollTop = containerRef.current.scrollHeight;
  }, [logs, autoScroll]);
  useLayoutEffect(() => {
    if (!focusEntry) return;
    const frame = requestAnimationFrame(() => {
      const container = containerRef.current;
      const row = container?.querySelector<HTMLElement>('[data-focused-log]');
      if (!container || !row) return;
      container.scrollTop += row.getBoundingClientRect().top - container.getBoundingClientRect().top - container.clientHeight / 2;
      row.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [focusEntry]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(logs.map((log) => `[${log.stream}] ${cleanTerminalText(log.text)}`).join('\n'));
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
      toast.success('日志已复制');
    } catch (error) {
      toast.error(`复制日志失败：${error}`);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="flex h-[640px] max-h-[85vh] max-w-4xl flex-col gap-0 overflow-hidden p-0" data-terminal-logs>
        <DialogHeader className="shrink-0 px-6 pb-4 pt-6 pr-12">
          <DialogTitle>{project.displayName || project.name}</DialogTitle>
          <DialogDescription>项目启动日志 · 仅保留本次运行的输出</DialogDescription>
        </DialogHeader>
        <div className="flex shrink-0 items-center gap-2 h1-form-footer px-5 py-2">
          <Badge variant="outline" className="text-muted-foreground">{logs.length} 行</Badge>
          <div className="ml-auto flex items-center gap-1">
            {!autoScroll && <Button size="sm" variant="outline" onClick={() => setAutoScroll(true)}><ArrowDown data-icon="inline-start" />到底部</Button>}
            <ActionTooltip label={copied ? '已复制' : '复制全部日志'}><Button size="icon-sm" variant="outline" aria-label="复制全部日志" onClick={handleCopy}>{copied ? <Check /> : <Copy />}</Button></ActionTooltip>
            <ActionTooltip label="清空日志"><Button size="icon-sm" variant="outline" aria-label="清空日志" onClick={onClear}><Trash2 /></Button></ActionTooltip>
          </div>
        </div>
        <div ref={containerRef} onScroll={() => {
          if (!containerRef.current) return;
          const { scrollTop, scrollHeight, clientHeight } = containerRef.current;
          setAutoScroll(scrollHeight - scrollTop - clientHeight < 40);
        }} className="mx-4 mb-4 min-h-0 flex-1 overflow-y-auto rounded-[14px] bg-terminal p-4 font-mono text-xs leading-relaxed text-terminal-foreground select-text">
          {logs.length === 0 ? <div className="flex h-full flex-col items-center justify-center gap-3 text-terminal-muted select-none"><Terminal className="size-7" /><span>暂无启动日志</span></div> : logs.map((log) => (
            <div key={log.id} data-focused-log={log.id === focusEntry || undefined} tabIndex={log.id === focusEntry ? -1 : undefined}
              className="break-all data-[focused-log=true]:bg-destructive/20" style={{ color: log.stream === 'stderr' ? TERMINAL_THEME.red : log.stream === 'system' ? TERMINAL_THEME.brightBlack : undefined }}>
              <span className="mr-3 text-terminal-muted select-none">{new Date(log.timestamp).toLocaleTimeString()}</span>{cleanTerminalText(log.text)}
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
