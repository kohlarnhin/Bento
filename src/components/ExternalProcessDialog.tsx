import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, Loader2 } from 'lucide-react';
import type { PortOwner, ProjectConfig } from '../types';
import Button from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Badge } from './ui/badge';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

export function ExternalProcessDialog({ project, owners, restart, onClose, onCompleted }: {
  project: ProjectConfig;
  owners: PortOwner[];
  restart: boolean;
  onClose: () => void;
  onCompleted: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const confirm = async () => {
    if (busy) return;
    setBusy(true); setError('');
    try {
      await invoke('stop_external_process', { id: project.id, pids: owners.map(owner => owner.pid) });
      if (restart) await invoke('start_project', { id: project.id });
      await onCompleted();
    } catch (error) {
      setError(String(error));
    } finally { setBusy(false); }
  };
  return (
    <Dialog open onOpenChange={open => { if (!open && !busy) onClose(); }}>
      <DialogContent className="flex max-h-[calc(100dvh-3rem)] flex-col" hideClose={busy} data-external-stop onOpenAutoFocus={event => {
        event.preventDefault(); document.querySelector<HTMLButtonElement>('[data-external-cancel]')?.focus();
      }}>
        <DialogHeader>
          <DialogTitle>结束外部进程？</DialogTitle>
          <DialogDescription>端口 :{project.port} 由不是 Bento 启动的进程占用。</DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-3 overflow-y-auto">
          {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
          {owners.map(owner => <div key={owner.pid} className="space-y-2 rounded-lg border bg-muted/30 p-3" data-port-owner={owner.pid}>
            <p className="truncate select-text font-mono text-xs" title={`${owner.pid} · ${owner.command}`}>{owner.pid} · {owner.command || '未知命令'}</p>
            <p className="truncate select-text text-xs text-muted-foreground" title={owner.cwd || undefined}>目录 {owner.cwd || '未知'}</p>
            {owner.inProject ? <Badge variant="secondary">看起来是该项目的进程</Badge> : <p className="flex items-start gap-1.5 text-xs leading-relaxed text-warning-foreground"><AlertCircle className="mt-0.5 size-3.5 shrink-0" />该进程不在项目目录中运行，可能是其他程序</p>}
          </div>)}
        </div>
        <div className="flex shrink-0 justify-end gap-2 pt-1">
          <Button variant="outline" onClick={onClose} disabled={busy} data-external-cancel>取消</Button>
          <Button variant="destructive" disabled={busy} onClick={() => { void confirm(); }} data-external-confirm>{busy && <Loader2 className="animate-spin" />}{busy ? '正在结束…' : restart ? '结束并重启' : '结束进程'}</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
