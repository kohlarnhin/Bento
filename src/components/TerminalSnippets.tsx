import { useCallback, useEffect, useState } from 'react';
import { Copy, Pencil, Plus, Trash2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import type { Snippet, SnippetChange } from '../types';
import { SidebarMenu } from './SidebarMenu';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Input } from './ui/input';
import { Textarea } from './ui/textarea';
import { MotionPresence } from './ui/motion-presence';

function SnippetEditor({ projectId, entry, existing, onChange, onClose }: {
  projectId: string;
  entry: Snippet | null;
  existing: Snippet[];
  onChange: (change: SnippetChange) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(entry?.name ?? '');
  const [command, setCommand] = useState(entry?.command ?? '');
  const [suggestions, setSuggestions] = useState<Pick<Snippet, 'name' | 'command'>[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (entry) return;
    let active = true;
    void invoke<typeof suggestions>('suggest_snippets', { projectId }).then(items => {
      if (active) setSuggestions(items.filter(item => !existing.some(old => old.command === item.command)));
    }).catch(() => {});
    return () => { active = false; };
  }, [entry, projectId, existing]);

  return <Dialog open onOpenChange={open => { if (!open && !saving) onClose(); }}>
    <DialogContent className="terminal-chrome max-w-lg" data-snippet-editor>
      <DialogHeader>
        <DialogTitle>{entry ? '编辑常用命令' : '添加常用命令'}</DialogTitle>
        <DialogDescription>此片段保存在当前项目中。</DialogDescription>
      </DialogHeader>
      <form className="grid gap-3" onSubmit={async event => {
        event.preventDefault();
        if (!command.trim() || saving) return;
        setSaving(true);
        setError('');
        try {
          await onChange(entry ? { action: 'edit', id: entry.id, name, command } : { action: 'add', name, command });
          onClose();
        } catch (error) { setError(String(error)); }
        finally { setSaving(false); }
      }}>
        <label className="grid gap-1 text-sm">名称（可选）
          <Input value={name} maxLength={40} disabled={saving} onChange={event => setName(event.target.value)} />
        </label>
        <label className="grid gap-1 text-sm">命令
          <Textarea value={command} rows={3} required disabled={saving} className="font-mono"
            data-snippet-command onChange={event => setCommand(event.target.value)} />
        </label>
        {suggestions.length > 0 && <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-xs text-muted-foreground">项目脚本</span>
          {suggestions.map(item => <Button key={item.command} type="button" variant="outline" size="sm"
            disabled={saving} title={item.command} onClick={() => { setName(item.name); setCommand(item.command); }}>{item.name}</Button>)}
        </div>}
        {error && <p role="alert" className="text-sm text-muted-foreground">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" disabled={saving} onClick={onClose}>取消</Button>
          <Button type="submit" disabled={!command.trim() || saving}>保存</Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>;
}

export function TerminalSnippets({ projectId, entries, onRun, onChange }: {
  projectId: string;
  entries: Snippet[];
  onRun: (command: string) => void;
  onChange: (change: SnippetChange) => Promise<void>;
}) {
  const [editor, setEditor] = useState<Snippet | null>();
  const [menu, setMenu] = useState<{ entry: Snippet; x: number; y: number }>();
  const closeMenu = useCallback(() => setMenu(undefined), []);
  return <>
    <div className="terminal-snippets" data-terminal-snippets>
      <span>常用命令</span>
      <div className="terminal-snippet-scroll">
        {entries.map(entry => <button type="button" key={entry.id} className="terminal-snippet" title={entry.command}
          data-snippet-id={entry.id} onClick={() => onRun(entry.command)}
          onContextMenu={event => { event.preventDefault(); setMenu({ entry, x: event.clientX, y: event.clientY }); }}>
          {entry.name || entry.command}
        </button>)}
        <button type="button" className="terminal-snippet terminal-snippet-add" aria-label="添加常用命令"
          disabled={entries.length >= 20} data-add-snippet onClick={() => setEditor(null)}><Plus size={14} aria-hidden="true" /></button>
      </div>
    </div>
    <MotionPresence>{menu && <SidebarMenu x={menu.x} y={menu.y} projectContext label="常用命令操作" className="terminal-chrome" onClose={closeMenu}
      items={[
        { label: '编辑', icon: <Pencil />, onSelect: () => setEditor(menu.entry) },
        { label: '复制', icon: <Copy />, onSelect: () => { void navigator.clipboard.writeText(menu.entry.command).catch(error => toast.error(String(error))); } },
        { label: '删除', icon: <Trash2 />, destructive: true, separatorBefore: true,
          onSelect: () => { void onChange({ action: 'delete', id: menu.entry.id }).catch(error => toast.error(String(error))); } },
      ]} />}</MotionPresence>
    <MotionPresence>{editor !== undefined && <SnippetEditor projectId={projectId} entry={editor}
      existing={entries} onChange={onChange} onClose={() => setEditor(undefined)} />}</MotionPresence>
  </>;
}
