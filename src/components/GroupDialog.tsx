import { useState } from 'react';
import type { ProjectGroup } from '../types';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Input } from './ui/input';
import { Label } from './ui/label';

export function GroupDialog({ group, deleting, onClose, onSave }: {
  group?: ProjectGroup; deleting?: boolean; onClose: () => void; onSave: (name: string) => Promise<void>;
}) {
  const [name, setName] = useState(group?.name ?? '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  return <Dialog open onOpenChange={open => { if (!open && !saving) onClose(); }}>
    <DialogContent className="max-w-sm" data-group-dialog hideClose={saving}>
      <DialogHeader>
        <DialogTitle>{deleting ? '删除分组' : group ? '重命名分组' : '新建分组'}</DialogTitle>
        <DialogDescription>{deleting ? `「${group?.name}」中的项目会移回未分组。` : '用分组整理你的项目，拖动标题可调整顺序。'}</DialogDescription>
      </DialogHeader>
      <form className="space-y-4" onSubmit={event => {
        event.preventDefault();
        if (saving || (!deleting && !name.trim())) return;
        setSaving(true);
        setError('');
        void onSave(name.trim()).then(onClose).catch(error => setError(String(error))).finally(() => setSaving(false));
      }} onKeyDown={event => {
        if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.keyCode === 229)) event.preventDefault();
      }}>
        {!deleting && <div className="h1-group space-y-2 p-3"><Label htmlFor="group-name">分组名称</Label><Input id="group-name" autoFocus value={name} maxLength={40} disabled={saving} onChange={event => setName(event.target.value)} /></div>}
        {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
        <div className="flex justify-end gap-2"><Button type="button" variant="outline" disabled={saving} onClick={onClose}>取消</Button>
          <Button type="submit" variant={deleting ? 'destructive' : 'default'} disabled={saving || (!deleting && !name.trim())}>{saving ? '保存中…' : deleting ? '删除分组' : '保存'}</Button></div>
      </form>
    </DialogContent>
  </Dialog>;
}
