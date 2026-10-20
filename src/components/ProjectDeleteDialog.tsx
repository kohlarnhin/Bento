import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig } from '../types';
import Button from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

export function ProjectDeleteDialog({ project, onClose, onDeleted }: {
  project: ProjectConfig;
  onClose: () => void;
  onDeleted: () => Promise<void>;
}) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState('');
  const remove = async () => {
    setDeleting(true);
    setError('');
    try {
      await invoke('delete_project', { id: project.id });
      toast.success('已删除项目');
      await onDeleted();
    } catch (error) {
      setError(String(error));
    } finally {
      setDeleting(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => { if (!open && !deleting) onClose(); }}>
      <DialogContent hideClose={deleting} data-project-delete onOpenAutoFocus={(event) => {
        event.preventDefault();
        document.querySelector<HTMLButtonElement>('[data-delete-cancel]')?.focus();
      }}>
        <DialogHeader>
          <DialogTitle className="break-words pr-4 leading-relaxed">删除项目「{project.displayName || project.name}」？</DialogTitle>
          <DialogDescription className="leading-relaxed">将从 Bento 中移除此项目，停止由 Bento 启动的进程并关闭该项目的终端。项目目录中的文件不会被删除。</DialogDescription>
        </DialogHeader>
        {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onClose} disabled={deleting} data-delete-cancel>取消</Button>
          <Button variant="destructive" onClick={() => { void remove(); }} disabled={deleting} data-delete-confirm>{deleting && <Loader2 className="animate-spin" />}{deleting ? '删除中…' : '删除'}</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
