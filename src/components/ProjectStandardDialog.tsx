import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, Copy, FileCode2, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import type { Service, StandardScript } from '../types';
import Button from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

export interface StandardRequest {
  path: string;
  services: Service[];
  mode: 'view' | 'generate';
}

export function ProjectStandardDialog({ request, onClose, onGenerated }: {
  request: StandardRequest;
  onClose: () => void;
  onGenerated?: (command: string) => void;
}) {
  const [preview, setPreview] = useState<StandardScript>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const code = useRef<HTMLTextAreaElement>(null);
  const generating = request.mode === 'generate';
  const { path, services } = request;

  useEffect(() => {
    let active = true;
    setPreview(undefined);
    setError('');
    void invoke<StandardScript>('preview_project_standard_script', { path, services })
      .then(script => { if (active) setPreview(script); })
      .catch(reason => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, [path, services, revision]);

  const copy = async () => {
    if (!preview) return;
    try {
      await navigator.clipboard.writeText(preview.script);
      toast.success('已复制脚本');
    } catch {
      code.current?.focus();
      code.current?.select();
      if (document.execCommand('copy')) toast.success('已复制脚本');
      else toast('已选中脚本，按 ⌘C 复制');
    }
  };

  const generate = async () => {
    if (!generating || !preview || busy) return;
    setBusy(true);
    try {
      const command = await invoke<string>('generate_project_standard_script', { path: request.path, services: request.services, preview });
      onGenerated?.(command);
      toast.success('已生成 ' + preview.relativePath);
      onClose();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose(); }}>
    <DialogContent data-standard-dialog={request.mode} hideClose={busy}
      className="flex max-h-[calc(100dvh-4rem)] max-w-[700px] flex-col gap-4 p-6">
      <DialogHeader>
        <DialogTitle>{generating ? '生成标准启动脚本' : 'Bento 启动标准模板'}</DialogTitle>
        <DialogDescription>{generating ? '确认后写入项目目录，并将脚本填入启动命令。保存项目后才应用新命令。'
          : '复制脚本后，按项目需要调整启动命令。前端可读取 API_PORT 配置代理。'}</DialogDescription>
      </DialogHeader>
      {error && <Alert variant="destructive"><AlertCircle /><AlertDescription>{error}</AlertDescription></Alert>}
      {preview ? <>
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
            <FileCode2 className="size-4 shrink-0" />
            <code className="break-all" data-standard-script-path>{preview.relativePath}</code>
          </div>
          {generating && !!preview.preserved.length && <p className="text-xs text-muted-foreground w-full">
            保留已有文件 {preview.preserved.join('、')}
          </p>}
        </div>
        <textarea ref={code} readOnly spellCheck={false} wrap="off" aria-label="启动脚本预览" data-standard-script-preview
          value={preview.script} className="h1-group min-h-[180px] w-full flex-1 resize-none p-3 font-mono text-xs leading-5 select-text outline-none focus-visible:ring-2 focus-visible:ring-ring/25"
          rows={18} />
        {!!preview.notes.length && <ul className="max-h-24 shrink-0 space-y-1 overflow-y-auto text-xs text-warning-foreground">
          {preview.notes.map(note => <li key={note}>{note}</li>)}
        </ul>}
      </> : !error && <p className="flex items-center gap-2 py-8 text-sm text-muted-foreground" role="status">
        <Loader2 className="size-4 animate-spin" />正在生成预览…
      </p>}
      <div className="flex shrink-0 flex-wrap justify-end gap-2 pt-1">
        <Button type="button" variant="outline" onClick={onClose} disabled={busy}>{generating ? '取消' : '关闭'}</Button>
        {error && <Button type="button" variant="outline" onClick={() => setRevision(current => current + 1)}>重新预览</Button>}
        {preview && <Button type="button" variant={generating ? 'outline' : 'default'} disabled={busy} onClick={() => void copy()} data-copy-standard>
          <Copy data-icon="inline-start" />复制脚本
        </Button>}
        {generating && <Button type="button" disabled={!preview || !!error || busy} onClick={() => void generate()} data-confirm-standard-script>
          {busy ? <Loader2 className="animate-spin" /> : <FileCode2 data-icon="inline-start" />}确认生成脚本
        </Button>}
      </div>
    </DialogContent>
  </Dialog>;
}
