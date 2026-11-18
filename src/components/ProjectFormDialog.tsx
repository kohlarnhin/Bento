import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { homeDir } from '@tauri-apps/api/path';
import { open } from '@tauri-apps/plugin-dialog';
import { AlertCircle, AlertTriangle, Check, FolderOpen, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import type { PortConflictCheck, ProjectConfig, ProjectDirInspection, ProjectDraft, ProjectGroup, ProjectStatus } from '../types';
import { getTerminalSnapshot } from '../lib/terminalRegistry';
import { cn } from '../lib/utils';
import { hasAdvancedErrors, validateAdvanced } from '../lib/projectSettings';
import type { AdvancedErrors } from '../lib/projectSettings';
import { ProjectAdvancedFields } from './ProjectAdvancedFields';
import Button from './ui/button';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Textarea } from './ui/textarea';
import { Alert, AlertDescription } from './ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

interface Props {
  project?: ProjectConfig;
  initialInspection?: ProjectDirInspection;
  projects: ProjectConfig[];
  groups: ProjectGroup[];
  onCreateGroup: (name: string) => Promise<ProjectGroup>;
  status?: ProjectStatus;
  onClose: () => void;
  onSaved: (project: ProjectConfig) => Promise<void>;
}
type Values = Omit<ProjectDraft, 'port' | 'groupId'> & { port: string; displayName: string; description: string };
type Field = Exclude<keyof Values, 'env' | 'commands' | 'autoOpen'>;
type Errors = Partial<Record<Field, string>>;

export function ProjectFormDialog({ project, initialInspection, projects, groups, onCreateGroup, status, onClose, onSaved }: Props) {
  const [groupId, setGroupId] = useState(project?.groupId ?? '');
  const [newGroupName, setNewGroupName] = useState('');
  const [groupSaving, setGroupSaving] = useState(false);
  const [groupError, setGroupError] = useState('');
  const createGroup = async () => {
    if (groupSaving || !newGroupName.trim()) return;
    setGroupSaving(true);
    setGroupError('');
    try { const group = await onCreateGroup(newGroupName.trim()); setGroupId(group.id); setNewGroupName(''); }
    catch (error) { setGroupError(String(error)); }
    finally { setGroupSaving(false); }
  };
  const [values, setValues] = useState<Values>(() => ({
    path: project?.path ?? initialInspection?.path ?? '', name: project?.name ?? '',
    displayName: project?.displayName ?? '', description: project?.description ?? '',
    type: project?.type === 'app' ? 'app' : 'web', port: project?.port?.toString() ?? '', startCommand: project?.startCommand ?? '',
    env: project?.env ?? [], commands: project?.commands ?? [], autoOpen: project?.autoOpen ?? false,
  }));
  const [advancedOpen, setAdvancedOpen] = useState(!!(project?.env?.length || project?.commands?.length || project?.autoOpen));
  const [advancedErrors, setAdvancedErrors] = useState<AdvancedErrors>({ env: [], commands: [] });
  const [inspection, setInspection] = useState<ProjectDirInspection>();
  const [errors, setErrors] = useState<Errors>({});
  const [saveError, setSaveError] = useState('');
  const [portCheck, setPortCheck] = useState<{ port: number; result: PortConflictCheck }>();
  const [portCheckError, setPortCheckError] = useState('');
  const [saving, setSaving] = useState(false);
  const edited = useRef<Partial<Record<Field, boolean>>>({});
  const inspected = useRef<{ path: string; result: ProjectDirInspection }>();
  const inspectionVersion = useRef(0);
  const pickedPath = useRef('');
  const pathRef = useRef(values.path);
  pathRef.current = values.path;
  const composing = useRef(false);
  const form = useRef<HTMLFormElement>(null);

  const inspect = useCallback(async (path: string) => {
    const version = ++inspectionVersion.current;
    const result = initialInspection && path === initialInspection.path
      ? initialInspection
      : await invoke<ProjectDirInspection>('inspect_project_dir', { path, excludeId: project?.id ?? null });
    if (version !== inspectionVersion.current || path !== pathRef.current) return result;
    inspected.current = { path, result };
    setInspection(result);
    if (!project && result.isDir) {
      setValues((current) => ({
        ...current,
        ...(!edited.current.name ? { name: result.name } : {}),
        ...(!edited.current.type ? { type: result.projectType } : {}),
        ...(!edited.current.startCommand ? { startCommand: result.commands[0] ?? '' } : {}),
        ...(!edited.current.port ? { port: result.port?.toString() ?? '' } : {}),
      }));
    }
    return result;
  }, [initialInspection, project]);

  useEffect(() => {
    setInspection(undefined);
    const timer = setTimeout(() => {
      if (values.path.trim()) void inspect(values.path).catch((error) => setErrors((previous) => ({ ...previous, path: `检查目录失败：${error}` })));
    }, initialInspection?.path === values.path || pickedPath.current === values.path ? 0 : 300);
    return () => { clearTimeout(timer); ++inspectionVersion.current; };
  }, [values.path, inspect, initialInspection]);

  useEffect(() => {
    setPortCheck(undefined);
    setPortCheckError('');
    const port = Number(values.port);
    if (!/^\d+$/.test(values.port.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) return;
    let active = true;
    const timer = setTimeout(() => {
      void invoke<PortConflictCheck>('check_port_conflicts', { port, excludeProjectId: project?.id ?? null })
        .then((result) => { if (active) setPortCheck({ port, result }); })
        .catch((error) => { if (active) setPortCheckError(`检查端口失败：${error}，保存时将再次校验项目端口`); });
    }, 300);
    return () => { active = false; clearTimeout(timer); };
  }, [values.port, project?.id]);

  const checkedPort = portCheck?.port === Number(values.port) ? portCheck.result : undefined;
  const configuredProject = projects.find((item) => item.id !== project?.id && item.port === Number(values.port));
  const projectConflict = checkedPort ? checkedPort.projectConflict : configuredProject && { id: configuredProject.id, name: configuredProject.displayName || configuredProject.name };
  const portConflictMessage = projectConflict ? `与「${projectConflict.name}」的端口 ${Number(values.port)} 重复` : undefined;

  const change = (field: Field, value: string) => {
    edited.current[field] = true;
    setValues((current) => ({ ...current, [field]: value, ...((field === 'type' && value !== 'web') || (field === 'port' && !value.trim()) ? { autoOpen: false } : {}) }));
    setErrors((current) => ({ ...current, [field]: undefined, ...(field === 'type' ? { port: undefined } : {}) }));
  };
  const validate = (current: Values, directory?: ProjectDirInspection): Errors => {
    const next: Errors = {};
    if (!current.path.trim()) next.path = '请输入项目目录';
    else if (directory && !directory.exists) next.path = '目录不存在';
    else if (directory && !directory.isDir) next.path = '不是文件夹';
    if (!current.name.trim()) next.name = '请输入项目名称';
    if (!current.startCommand.trim()) next.startCommand = '请输入启动命令';
    if (!current.port.trim()) {
      if (current.type === 'web') next.port = 'Web 项目需要填写端口';
    } else {
      const port = Number(current.port);
      if (!/^\d+$/.test(current.port.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) next.port = '端口必须是 1–65535 之间的整数';
      else if (projectConflict) next.port = portConflictMessage;
    }
    return next;
  };
  const blur = async (field: Field) => {
    try {
      const directory = field === 'path' && values.path.trim()
        ? inspected.current?.path === values.path ? inspected.current.result : await inspect(values.path)
        : inspection;
      if (field === 'path' && values.path !== pathRef.current) return;
      setErrors((current) => ({ ...current, [field]: validate(values, directory)[field] }));
    } catch (error) {
      setErrors((current) => ({ ...current, [field]: `检查目录失败：${error}` }));
    }
  };
  const pickFolder = async () => {
    try {
      const home = (await homeDir()).replace(/\/$/, '');
      const path = values.path.trim();
      const defaultPath = path === '~' ? home : path.startsWith('~/') ? home + path.slice(1) : path || home;
      const selected = await open({ directory: true, multiple: false, defaultPath });
      if (typeof selected === 'string') {
        pickedPath.current = selected;
        pathRef.current = selected;
        change('path', selected);
        await inspect(selected);
      }
    } catch (error) {
      setErrors((current) => ({ ...current, path: `选择目录失败：${error}` }));
    }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (composing.current || saving) return;
    if (groupId === '__new__') { setGroupError('请先创建分组，或选择已有分组'); return; }
    setSaving(true);
    setSaveError('');
    try {
      const directory = values.path.trim()
        ? inspected.current?.path === values.path ? inspected.current.result : await inspect(values.path)
        : undefined;
      const next = validate(values, directory);
      const advanced = validateAdvanced(values.env, values.commands);
      setErrors(next);
      setAdvancedErrors(advanced);
      if (hasAdvancedErrors(advanced)) setAdvancedOpen(true);
      if (Object.values(next).some(Boolean) || hasAdvancedErrors(advanced)) {
        requestAnimationFrame(() => {
          const invalid = form.current?.querySelector<HTMLElement>('[aria-invalid="true"]');
          // Keep the field's label visible when the body is scrolled.
          invalid?.closest('.space-y-2')?.scrollIntoView({ block: 'nearest' });
          invalid?.focus({ preventScroll: true });
        });
        return;
      }
      const draft: ProjectDraft = { ...values, port: values.port.trim() ? Number(values.port) : null, groupId: groupId || null };
      const saved = await invoke<ProjectConfig>(project ? 'update_project' : 'add_project', { ...(project ? { id: project.id } : {}), project: draft });
      toast.success(project ? '已保存修改' : `已添加项目「${saved.displayName || saved.name}」`);
      if (project && saved.path !== project.path && Object.values(getTerminalSnapshot()).some(terminal => terminal.projectId === project.id && terminal.alive)) toast('终端仍在原目录，可在侧边栏重启终端');
      await onSaved(saved);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      setSaving(false);
    }
  };
  const fieldProps = (field: Field) => ({
    id: `project-${field}`, name: field, value: values[field], disabled: saving,
    onChange: (event: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => change(field, event.target.value),
    onBlur: () => { void blur(field); }, 'aria-invalid': !!errors[field] || (field === 'port' && !!projectConflict),
    'aria-describedby': `project-${field}-message${field === 'port' ? ' project-port-warnings' : ''}`,
  });
  const message = (field: Field, hint?: string) => (
    <p id={`project-${field}-message`} className={cn('text-[11px] leading-relaxed', errors[field] || (field === 'port' && portConflictMessage) ? 'text-destructive' : 'text-muted-foreground')} aria-live="polite">
      {errors[field] || (field === 'port' && portConflictMessage) || hint}
    </p>
  );
  const source = inspection?.portSource;
  const explicitPort = source && source !== '推荐空闲端口' && !source.endsWith('默认端口');
  const portHint = !project && !edited.current.port && source
    ? `${explicitPort ? `从 ${source} 检测` : source}${!explicitPort ? '，请确认项目实际监听该端口' : ''}`
    : values.type === 'app' ? '可选，填写应用开发服务器的实际端口' : '须与项目实际监听的端口一致，修改后请同步启动命令或项目配置';
  const hasPortWarning = !!(projectConflict || checkedPort?.inUse || checkedPort?.reserved.length);

  return (
    <Dialog open onOpenChange={(isOpen) => { if (!isOpen && !saving) onClose(); }}>
      <DialogContent className="flex max-h-[calc(100dvh-3rem)] max-w-[620px] flex-col gap-0 overflow-hidden p-0" hideClose={saving} data-project-form>
        <form ref={form} onSubmit={submit} noValidate className="flex min-h-0 flex-col overflow-hidden"
          onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
          onKeyDownCapture={(event) => {
            if (event.key === 'Enter' && (composing.current || event.nativeEvent.isComposing || event.keyCode === 229)) {
              event.preventDefault();
              event.stopPropagation();
            }
          }}>
          <DialogHeader className="shrink-0 px-6 pb-4 pt-6">
            <DialogTitle>{project ? '编辑项目' : '新建项目'}</DialogTitle>
            <DialogDescription>{project ? '调整本地目录与运行配置。' : '选择本地项目目录，自动识别运行配置。'}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 space-y-4 overflow-y-auto px-6 pb-5 pt-1" data-project-form-body>
            {project && (status === 'running' || status === 'starting') && <Alert><AlertDescription>项目正在运行，修改将在重启后生效</AlertDescription></Alert>}
            {saveError && <Alert variant="destructive"><AlertCircle /><AlertDescription>{saveError}</AlertDescription></Alert>}
            <div className="space-y-2">
              <Label htmlFor="project-path">项目目录 <span className="text-destructive">*</span></Label>
              <div className="flex gap-2">
                <Input {...fieldProps('path')} placeholder="~/Projects/my-project" autoFocus className="font-mono text-xs" />
                <Button type="button" variant="outline" onClick={() => { void pickFolder(); }} disabled={saving}><FolderOpen />选择…</Button>
              </div>
              {message('path', '支持 ~，也可以从 Finder 拖入文件夹到侧边栏')}
              {inspection?.existingProject && <p className="text-[11px] text-warning-foreground" role="status">此目录已用于项目「{inspection.existingProject.name}」，仍可继续添加。</p>}
            </div>
            <div className="space-y-2"><Label htmlFor="project-name">名称 <span className="text-destructive">*</span></Label><Input {...fieldProps('name')} placeholder="项目名称" />{errors.name && message('name')}</div>
            <div className="space-y-2"><Label htmlFor="project-displayName">显示名称 <span className="font-normal text-muted-foreground">（可选）</span></Label><Input {...fieldProps('displayName')} placeholder="在侧边栏显示的名称" /></div>
            <div className="space-y-2">
              <Label htmlFor="project-group">分组</Label>
              <select id="project-group" value={groupId} disabled={saving || groupSaving}
                onChange={event => { setGroupId(event.target.value); setGroupError(''); }}
                className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50">
                <option value="">未分组</option>
                {groups.map(group => <option key={group.id} value={group.id}>{group.name}</option>)}
                <option value="__new__">＋ 新建分组…</option>
              </select>
              {groupId === '__new__' && <div className="flex gap-2">
                <Input aria-label="新分组名称" placeholder="分组名称" maxLength={40} value={newGroupName} disabled={saving || groupSaving}
                  onChange={event => setNewGroupName(event.target.value)} onKeyDown={event => {
                    if (event.key === 'Enter' && !event.nativeEvent.isComposing && event.keyCode !== 229) { event.preventDefault(); void createGroup(); }
                  }} />
                <Button type="button" variant="outline" disabled={saving || groupSaving || !newGroupName.trim()} onClick={() => { void createGroup(); }}>{groupSaving ? '创建中…' : '创建'}</Button>
              </div>}
              {groupError && <p role="alert" className="text-xs text-destructive">{groupError}</p>}
            </div>
            <div className="space-y-2"><Label htmlFor="project-description">描述 <span className="font-normal text-muted-foreground">（可选）</span></Label><Textarea {...fieldProps('description')} rows={2} className="resize-none" placeholder="项目用途或备注" /></div>
            <div className="space-y-2">
              <Label id="project-type-label">类型</Label>
              <div role="radiogroup" aria-labelledby="project-type-label" className="grid grid-cols-2 gap-1 rounded-lg border bg-muted/50 p-1"
                onKeyDown={(event) => {
                  if (saving || !['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
                  event.preventDefault();
                  const type = values.type === 'web' ? 'app' : 'web';
                  change('type', type);
                  event.currentTarget.querySelector<HTMLButtonElement>(`[data-type="${type}"]`)?.focus();
                }}>
                {(['web', 'app'] as const).map((type) => <button key={type} type="button" role="radio" data-type={type} aria-checked={values.type === type} tabIndex={values.type === type ? 0 : -1} disabled={saving} onClick={() => change('type', type)} className={cn('rounded-md px-3 py-2 text-xs outline-none transition-[color,background-color,box-shadow] focus-visible:ring-[3px] focus-visible:ring-ring/50', values.type === type ? 'bg-background font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground')}>{type === 'web' ? 'Web 项目' : '桌面应用'}</button>)}
              </div>
            </div>
            <div className="space-y-2">
              <Label htmlFor="project-port">端口 {values.type === 'web' ? <span className="text-destructive">*</span> : <span className="font-normal text-muted-foreground">（可选）</span>}</Label>
              <Input {...fieldProps('port')} inputMode="numeric" placeholder={values.type === 'web' ? '例如 5173' : '无开发服务器时可留空'} />
              {message('port', portHint)}
              <div id="project-port-warnings" className="space-y-2" role="status" aria-live="polite">
                {checkedPort?.inUse && <div className="flex gap-2 rounded-md border border-warning/20 bg-warning-muted p-2.5 text-[11px] text-warning-foreground">
                  <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                  <div className="min-w-0 space-y-1">
                    <p>此端口正被其他进程监听，仍可保存。</p>
                    {checkedPort.inUse.owners.map((owner) => <p key={owner.pid} className="break-words font-mono">PID {owner.pid} · {owner.command || '命令不可读'}</p>)}
                    {!checkedPort.inUse.owners.length && <p>无法读取监听进程的 PID / 命令。</p>}
                  </div>
                </div>}
                {checkedPort?.reserved.map((warning) => <p key={warning} className="flex items-start gap-2 text-[11px] leading-relaxed text-warning-foreground"><AlertTriangle className="mt-0.5 size-3.5 shrink-0" />{warning}</p>)}
                {portCheckError && <p className="text-[11px] text-warning-foreground">{portCheckError}</p>}
                {hasPortWarning && checkedPort?.suggestion != null && <Button type="button" variant="outline" size="sm" className="h-7 text-[11px]" disabled={saving} onClick={() => change('port', String(checkedPort.suggestion))}>改用 {checkedPort.suggestion}</Button>}
              </div>
              {!!inspection?.portCandidates.length && <div className="space-y-2 pt-1" role="group" aria-label="扫描到的候选端口">
                <p className="text-[11px] text-muted-foreground">扫描到的端口 · 点击填入</p>
                <div className="flex flex-wrap gap-1.5">
                  {inspection.portCandidates.map((candidate) => <Button key={candidate.port} type="button" variant="outline" size="sm" disabled={saving} aria-pressed={Number(values.port) === candidate.port}
                    onClick={() => change('port', String(candidate.port))}
                    className={cn('h-auto max-w-full items-start gap-2 px-2.5 py-2 text-left', Number(values.port) === candidate.port && 'border-primary/40 bg-primary/5')}>
                    <span className="min-w-0"><span className="flex items-center gap-1.5 font-mono text-xs">{candidate.port}{Number(values.port) === candidate.port && <Check className="!size-3" />}</span><span className="mt-0.5 block whitespace-normal break-words text-[10px] font-normal leading-relaxed text-muted-foreground">{candidate.source}</span></span>
                  </Button>)}
                </div>
              </div>}
            </div>
            <div className="space-y-2">
              <Label htmlFor="project-startCommand">启动命令 <span className="text-destructive">*</span></Label>
              <Input {...fieldProps('startCommand')} className="font-mono text-xs" placeholder="例如 pnpm dev" />
              {errors.startCommand && message('startCommand')}
              {!!inspection?.commands.length && <div className="flex flex-wrap gap-1.5">{inspection.commands.slice(0, 4).map((command) => <Button type="button" key={command} variant="outline" size="sm" className="h-6 px-2 font-mono text-[11px]" disabled={saving} onClick={() => change('startCommand', command)}>{command}</Button>)}</div>}
            </div>
            <ProjectAdvancedFields values={values} open={advancedOpen} onOpenChange={setAdvancedOpen} disabled={saving} canAutoOpen={values.type === 'web' && !!values.port.trim()}
              errors={advancedErrors} onBlur={() => setAdvancedErrors(validateAdvanced(values.env, values.commands))}
              onChange={patch => { setValues(current => ({ ...current, ...patch })); setAdvancedErrors({ env: [], commands: [] }); }} />
          </div>
          <footer className="flex shrink-0 justify-end gap-2 border-t bg-background px-6 py-4">
            <Button type="button" variant="outline" disabled={saving} onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || !!projectConflict} data-project-save>{saving && <Loader2 className="animate-spin" />}{saving ? '保存中…' : project ? '保存修改' : '添加项目'}</Button>
          </footer>
        </form>
      </DialogContent>
    </Dialog>
  );
}
