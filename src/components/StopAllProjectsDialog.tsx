import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig } from '../types';
import Button from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

export function StopAllProjectsDialog({ projects, onClose, onStopped }: {
  projects: ProjectConfig[];
  onClose: () => void;
  onStopped: () => Promise<void>;
}) {
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState('');
  const stop = async () => {
    setStopping(true);
    setError('');
    try {
      const ids = await invoke<string[]>('stop_all_projects');
      toast.success(`已停止 ${ids.length} 个项目`);
      await onStopped();
    } catch (error) {
      setError(String(error));
    } finally {
      setStopping(false);
    }
  };
  return (
    <Dialog open onOpenChange={open => { if (!open && !stopping) onClose(); }}>
      <DialogContent hideClose={stopping} data-stop-all-dialog onOpenAutoFocus={event => {
        event.preventDefault();
        document.querySelector<HTMLButtonElement>('[data-stop-all-cancel]')?.focus();
      }}>
        <DialogHeader>
          <DialogTitle>停止全部项目？</DialogTitle>
          <DialogDescription className="max-h-[50vh] overflow-y-auto break-words leading-relaxed">
            将停止以下由 Bento 启动的项目：{projects.map(project => project.displayName || project.name).join('、')}。外部启动的进程不受影响。
          </DialogDescription>
        </DialogHeader>
        {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onClose} disabled={stopping} data-stop-all-cancel>取消</Button>
          <Button variant="destructive" onClick={() => { void stop(); }} disabled={stopping || !projects.length} data-stop-all-confirm>{stopping && <Loader2 className="animate-spin" />}{stopping ? '停止中…' : '全部停止'}</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
