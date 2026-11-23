import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { homeDir } from '@tauri-apps/api/path';
import { open } from '@tauri-apps/plugin-dialog';
import { AlertCircle, AlertTriangle, Check, FileCode2, FolderOpen, Loader2, Plus, RotateCw } from 'lucide-react';
import { toast } from 'sonner';
import type { DashboardConflict, PortConflictCheck, ProjectConfig, ProjectDirInspection, ProjectDraft, ProjectGroup, ProjectStatus } from '../types';
import { getTerminalSnapshot } from '../lib/terminalRegistry';
import { cn } from '../lib/utils';
import { applyInspectionDiff, diffProjectInspection, type RescanField } from '../lib/projectRescan';
import { defaultServiceKey, normalizedServiceDir, portNumber, projectPorts, savedServices, serviceDraft, uniqueServiceKey, type ServiceDraft } from '../lib/projectServices';
import { useProjectLaunch } from '../lib/useProjectLaunch';
import { usePortStandard } from '../lib/usePortStandard';
import { hasAdvancedErrors, validateAdvanced } from '../lib/projectSettings';
import type { AdvancedErrors } from '../lib/projectSettings';
import { ProjectAdvancedFields } from './ProjectAdvancedFields';
import { MotionPresence } from './ui/motion-presence';
import { ProjectRescanCard } from './ProjectRescanCard';
import { ProjectServiceFields, type ServiceErrors, type ServiceField } from './ProjectServiceFields';
import { ProjectStandardDialog, type StandardRequest } from './ProjectStandardDialog';
import Button from './ui/button';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Select } from './ui/select';
import { SegmentedControl } from './ui/segmented-control';
import { Textarea } from './ui/textarea';
import { Alert, AlertDescription } from './ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';

interface Props {
  project?: ProjectConfig;
  initialInspection?: ProjectDirInspection;
  focusPort?: DashboardConflict;
  scanOnOpen?: boolean;
  projects: ProjectConfig[];
  groups: ProjectGroup[];
  onCreateGroup: (name: string) => Promise<ProjectGroup>;
  status?: ProjectStatus;
  onClose: () => void;
  onSaved: (project: ProjectConfig) => Promise<void>;
}
type Values = Omit<ProjectDraft, 'port' | 'services' | 'groupId'> & { port: string; services: ServiceDraft[]; displayName: string; description: string };
type Field = Exclude<keyof Values, 'env' | 'commands' | 'autoOpen' | 'services' | 'injectPorts' | 'runnable'>;
type Errors = Partial<Record<Field, string>>;
type Rescan = { status: 'scanning' } | { status: 'error'; message: string } |
  { status: 'ready'; result: ProjectDirInspection; unchecked: Set<RescanField> };
const updatedInput = 'border-[var(--selected-line)]';
const types = [
  { value: 'web', label: 'Web 项目', attributes: { 'data-type': 'web' } },
  { value: 'app', label: '桌面应用', attributes: { 'data-type': 'app' } },
] as const;

function validateServices(services: ServiceDraft[], conflictForPort: (port: number) => string | undefined): ServiceErrors {
  const ports = new Set<number>();
  const dirs = new Set<string>();
  const keys = new Set<string>();
  const errors: ServiceErrors = {};
  const independent = services.some(service => !!service.startCommand?.trim());
  for (const service of services) {
    const result: ServiceErrors[string] = {};
    if (independent && !service.startCommand?.trim()) result.startCommand = '请输入独立启动命令';
    if (!service.name.trim()) result.name = '请输入服务名称';
    const raw = service.dir.trim();
    const dir = normalizedServiceDir(raw);
    if (!raw || raw.startsWith('/') || raw.split('/').includes('..')) result.dir = '请输入项目内的相对目录';
    else if (dirs.has(dir)) result.dir = '服务目录重复';
    dirs.add(dir);
    if (!/^[A-Z0-9_]+$/.test(service.key)) result.key = '角色 key 只能使用大写字母、数字和下划线';
    else if (keys.has(service.key)) result.key = '服务角色 key ' + service.key + ' 重复';
    keys.add(service.key);
    const port = portNumber(service.port);
    if (port == null) {
      if (service.port.trim()) result.port = '端口必须是 1–65535 之间的整数';
    } else {
      const conflict = conflictForPort(port);
      if (ports.has(port)) result.port = `服务端口 ${port} 重复`;
      else if (conflict) result.port = conflict;
      ports.add(port);
    }
    if (Object.keys(result).length) errors[service.id] = result;
  }
  return errors;
}

function UpdatedBadge() {
  return <span className="service-primary-label ml-1.5">已更新</span>;
}

export function ProjectFormDialog({ project, initialInspection, focusPort, scanOnOpen, projects, groups, onCreateGroup, status, onClose, onSaved }: Props) {
  const [groupId, setGroupId] = useState(project?.groupId ?? '');
  const [groupSaving, setGroupSaving] = useState(false);
  const createGroup = async (name: string) => {
    setGroupSaving(true);
    try { setGroupId((await onCreateGroup(name)).id); }
    finally { setGroupSaving(false); }
  };
  const counts = new Map(groups.map(group => [group.id, 0]));
  counts.set('', 0);
  for (const item of projects) {
    if (item.archived) continue;
    const bucket = item.groupId && counts.has(item.groupId) ? item.groupId : '';
    if (bucket || !item.pinned) counts.set(bucket, (counts.get(bucket) ?? 0) + 1);
  }
  const groupOptions = [{ value: '', label: '未分组', count: counts.get('') },
    ...groups.map(group => ({ value: group.id, label: group.name, count: counts.get(group.id) }))];
  const [values, setValues] = useState<Values>(() => ({
    path: project?.path ?? initialInspection?.path ?? '', name: project?.name ?? '', runnable: project?.runnable ?? true,
    displayName: project?.displayName ?? '', description: project?.description ?? '',
    type: project?.type === 'app' ? 'app' : 'web', port: project?.port?.toString() ?? '', startCommand: project?.startCommand ?? '',
    env: project?.env ?? [], commands: project?.commands ?? [], autoOpen: project?.autoOpen ?? false,
    services: project?.services?.map(serviceDraft) ?? [], injectPorts: project?.injectPorts ?? true,
  }));
  const [advancedOpen, setAdvancedOpen] = useState(!!(project?.env?.length || project?.commands?.length || project?.autoOpen));
  const [advancedErrors, setAdvancedErrors] = useState<AdvancedErrors>({ env: [], commands: [] });
  const [inspection, setInspection] = useState<ProjectDirInspection>();
  const [rescan, setRescan] = useState<Rescan>();
  const [appliedServiceDiff, setAppliedServiceDiff] = useState<ReturnType<typeof diffProjectInspection>>();
  const [newServiceIds, setNewServiceIds] = useState<ReadonlySet<string>>(() => new Set());
  const [validateServiceFields, setValidateServiceFields] = useState(false);
  const [updated, setUpdated] = useState<Partial<Record<RescanField, boolean>>>({});
  const [errors, setErrors] = useState<Errors>({});
  const [saveError, setSaveError] = useState('');
  const [portChecks, setPortChecks] = useState<Record<number, PortConflictCheck>>({});
  const [portCheckError, setPortCheckError] = useState('');
  const [saving, setSaving] = useState(false);
  const [standardDialog, setStandardDialog] = useState<StandardRequest>();
  const edited = useRef<Partial<Record<Field, boolean>>>({});
  const editedServices = useRef(false);
  const editedServiceKeys = useRef(new Set<string>());
  const manualServiceIndex = useRef(0);
  const inspected = useRef<{ path: string; result: ProjectDirInspection }>();
  const inspectionVersion = useRef(0);
  const rescanVersion = useRef(0);
  const rescanButton = useRef<HTMLButtonElement>(null);
  const rescanCard = useRef<HTMLDivElement>(null);
  const pickedPath = useRef('');
  const pathRef = useRef(values.path);
  pathRef.current = values.path;
  const composing = useRef(false);
  const form = useRef<HTMLFormElement>(null);
  const runnable = values.runnable;
  const multiService = runnable && !!values.services.length;
  const hasServiceCommands = multiService && values.services.some(service => !!service.startCommand?.trim());
  const launchPreview = useProjectLaunch({
    path: values.path,
    startCommand: values.startCommand,
    port: portNumber(values.port),
    services: savedServices(values.services),
    injectPorts: values.injectPorts,
    env: values.env,
  }, runnable && (!!values.startCommand.trim() || hasServiceCommands));
  const checkedPortKey = (!runnable ? [] : multiService ? [...new Set(values.services.map(service => portNumber(service.port)))].sort()
    : [portNumber(values.port)]).filter(port => port != null).join(',');

  const inspect = useCallback(async (path: string) => {
    const version = ++inspectionVersion.current;
    const result = !project && initialInspection && path === initialInspection.path
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
        ...(!editedServices.current ? { services: (result.services?.length ?? 0) >= 2 ? result.services.map(serviceDraft) : [] } : {}),
      }));
    }
    return result;
  }, [initialInspection, project]);

  useEffect(() => {
    setInspection(undefined);
    const timer = setTimeout(() => {
      if (values.path.trim()) void inspect(values.path).catch((error) => {
        if (values.path === pathRef.current) setErrors((previous) => ({ ...previous, path: `检查目录失败：${error}` }));
      });
    }, initialInspection?.path === values.path || pickedPath.current === values.path ? 0 : 300);
    return () => { clearTimeout(timer); ++inspectionVersion.current; };
  }, [values.path, inspect, initialInspection]);

  useEffect(() => {
    setPortChecks({});
    setPortCheckError('');
    if (!checkedPortKey) return;
    const ports = checkedPortKey.split(',').map(Number);
    let active = true;
    const timer = setTimeout(() => {
      const excludeProjectId = project?.id ?? null;
      const request = multiService
        ? invoke<Record<number, PortConflictCheck>>('check_service_port_conflicts', { ports, excludeProjectId })
        : invoke<PortConflictCheck>('check_port_conflicts', { port: ports[0], excludeProjectId }).then(result => ({ [ports[0]]: result }));
      void request
        .then((result) => { if (active) setPortChecks(result); })
        .catch((error) => { if (active) setPortCheckError(`检查端口失败：${error}，保存时将再次校验项目端口`); });
    }, 300);
    return () => { active = false; clearTimeout(timer); };
  }, [checkedPortKey, multiService, project?.id]);

  const conflictForPort = (port: number) => {
    const configured = projects.find(item => item.id !== project?.id && projectPorts(item).includes(port));
    const name = portChecks[port]?.projectConflict?.name || (configured?.displayName || configured?.name);
    return name ? `与「${name}」的端口 ${port} 重复` : undefined;
  };
  const checkedPort = portChecks[portNumber(values.port) ?? 0];
  const portConflictMessage = runnable && !multiService ? conflictForPort(Number(values.port)) : undefined;
  const serviceConflicts: ServiceErrors = Object.fromEntries((multiService ? values.services : []).flatMap(service => {
    const port = portNumber(service.port);
    const conflict = port == null ? undefined : conflictForPort(port);
    return conflict ? [[service.id, { port: conflict }]] : [];
  }));
  const serviceErrors = validateServiceFields ? validateServices(multiService ? values.services : [], conflictForPort) : serviceConflicts;
  const canAutoOpen = runnable && values.type === 'web' && portNumber(values.services.find(service => service.openWeb)?.port ?? values.port) != null;

  const rescanResult = rescan?.status === 'ready' ? rescan.result : undefined;
  const rescanError = rescan?.status === 'error' ? rescan.message : undefined;
  const scanning = rescan?.status === 'scanning';
  const diff = rescanResult ? diffProjectInspection(values, rescanResult) : undefined;
  const selectedChanges = new Set<RescanField>(diff?.rows.filter(row => row.changed && rescan?.status === 'ready' && !rescan.unchecked.has(row.field)).map(row => row.field));
  if (diff?.serviceChanges.length && rescan?.status === 'ready' && !rescan.unchecked.has('services')) selectedChanges.add('services');
  const auditedConfig = diff ? applyInspectionDiff(diff.rows, selectedChanges) : undefined;
  const auditedServices = savedServices(diff && selectedChanges.has('services') ? diff.services : values.services);
  const auditedCommand = auditedConfig?.startCommand ?? values.startCommand;
  const showStandard = !!(diff || appliedServiceDiff) && values.injectPorts && auditedServices.length > 1
    && auditedServices.every(service => !service.startCommand?.trim());
  const standardCheck = usePortStandard(values.path, auditedCommand, auditedServices, showStandard);
  const unchanged = !!diff && !diff.serviceChanges.length && !diff.rows.some(row => row.changed);

  useEffect(() => () => { ++rescanVersion.current; }, []);
  useEffect(() => {
    if (rescanResult || rescanError) rescanCard.current?.scrollIntoView({ block: 'nearest' });
  }, [rescanResult, rescanError]);
  useEffect(() => {
    if (!unchanged || showStandard) return;
    const timer = setTimeout(() => setRescan(current => current?.status === 'ready' && current.result === rescanResult ? undefined : current), 2000);
    return () => clearTimeout(timer);
  }, [unchanged, rescanResult, showStandard]);

  const scanAgain = async () => {
    if (!project || scanning || saving || !values.runnable) return;
    const version = ++rescanVersion.current;
    const path = values.path;
    setAppliedServiceDiff(undefined);
    if (!path.trim()) { setRescan({ status: 'error', message: '重新扫描失败：请输入项目目录' }); return; }
    const isCurrent = () => version === rescanVersion.current && path === pathRef.current;
    setRescan({ status: 'scanning' });
    try {
      const result = await inspect(path);
      if (!isCurrent()) return;
      if (!result.exists || !result.isDir) {
        setRescan({ status: 'error', message: `重新扫描失败：${result.exists ? '不是文件夹' : '目录不存在'}` });
      } else {
        setRescan({ status: 'ready', result, unchecked: new Set() });
      }
    } catch (error) {
      if (isCurrent()) setRescan({ status: 'error', message: `重新扫描失败：${error}` });
    }
  };
  useEffect(() => {
    if (!scanOnOpen) return;
    const frame = requestAnimationFrame(() => { void scanAgain(); });
    return () => cancelAnimationFrame(frame);
  }, [scanOnOpen]);
  const dismissRescan = () => {
    setRescan(undefined);
    rescanButton.current?.focus({ preventScroll: true });
  };
  const applyRescan = () => {
    if (!diff || !selectedChanges.size || saving) return;
    const patch = applyInspectionDiff(diff.rows, selectedChanges);
    const applyServices = selectedChanges.has('services');
    setValues(current => ({ ...current, ...patch, ...(applyServices ? { services: diff.services } : {}) }));
    if (applyServices) {
      editedServices.current = true;
      setNewServiceIds(current => new Set([...current, ...diff.serviceChanges.filter(change => change.added).map(change => change.service.id)]));
    }
    if (applyServices || (diff.startScripts.length && selectedChanges.has('startCommand'))) setAppliedServiceDiff(diff);
    setErrors(current => ({ ...current, ...((patch.type || patch.port !== undefined) && { port: undefined }),
      ...(patch.startCommand !== undefined && { startCommand: undefined }) }));
    setUpdated(current => {
      const next = { ...current };
      for (const field of selectedChanges) next[field] = true;
      return next;
    });
    dismissRescan();
  };

  const change = (field: Field, value: string) => {
    if (field === 'path' && value !== pathRef.current) {
      pathRef.current = value;
      ++rescanVersion.current;
      setRescan(undefined);
      setAppliedServiceDiff(undefined);
    }
    if (field === 'type' || field === 'port' || field === 'startCommand') {
      setUpdated(current => current[field] ? { ...current, [field]: false } : current);
      setRescan(current => current?.status === 'ready' && !current.unchecked.has(field)
        ? { ...current, unchecked: new Set([...current.unchecked, field]) } : current);
    }
    edited.current[field] = true;
    setValues((current) => ({ ...current, [field]: value, ...((field === 'type' && value !== 'web') || (field === 'port' && !value.trim()) ? { autoOpen: false } : {}) }));
    setErrors((current) => ({ ...current, [field]: undefined, ...(field === 'type' ? { port: undefined } : {}) }));
  };
  const changeRunnable = (next: boolean) => {
    if (!next) {
      ++rescanVersion.current;
      setRescan(undefined);
      setAppliedServiceDiff(undefined);
    }
    setValues(current => ({ ...current, runnable: next }));
    setErrors(current => ({ ...current, port: undefined, startCommand: undefined }));
  };
  const validate = (current: Values, directory?: ProjectDirInspection): Errors => {
    const next: Errors = {};
    if (!current.path.trim()) next.path = '请输入项目目录';
    else if (directory && !directory.exists) next.path = '目录不存在';
    else if (directory && !directory.isDir) next.path = '不是文件夹';
    if (!current.name.trim()) next.name = '请输入项目名称';
    if (!current.runnable) return next;
    if (!current.startCommand.trim() && !current.services.some(service => service.startCommand?.trim())) next.startCommand = '请输入启动命令';
    if (!current.services.length && current.port.trim()) {
      if (portNumber(current.port) == null) next.port = '端口必须是 1–65535 之间的整数';
      else if (portConflictMessage) next.port = portConflictMessage;
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
        change('path', selected);
        await inspect(selected);
      }
    } catch (error) {
      setErrors((current) => ({ ...current, path: `选择目录失败：${error}` }));
    }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (composing.current || saving || groupSaving) return;
    setSaving(true);
    setSaveError('');
    try {
      const directory = values.path.trim()
        ? inspected.current?.path === values.path ? inspected.current.result : await inspect(values.path)
        : undefined;
      const next = validate(values, directory);
      const advanced = validateAdvanced(values.env, values.commands);
      const invalidServices = validateServices(values.runnable ? values.services : [], conflictForPort);
      setErrors(next);
      setValidateServiceFields(true);
      setAdvancedErrors(advanced);
      if (hasAdvancedErrors(advanced)) setAdvancedOpen(true);
      if (Object.values(next).some(Boolean) || Object.keys(invalidServices).length || hasAdvancedErrors(advanced)) {
        requestAnimationFrame(() => {
          const invalid = form.current?.querySelector<HTMLElement>('[aria-invalid="true"]');
          const details = invalid?.closest('details');
          if (details) details.open = true;
          // Keep the field's label visible when the body is scrolled.
          invalid?.closest('.space-y-2')?.scrollIntoView({ block: 'nearest' });
          invalid?.focus({ preventScroll: true });
        });
        return;
      }
      const services = values.runnable ? savedServices(values.services) : [];
      const port = !values.runnable ? null : services.length ? services.find(service => service.openWeb)?.port ?? null : portNumber(values.port);
      const draft: ProjectDraft = { ...values, services, port, startCommand: values.runnable ? values.startCommand : '',
        groupId: groupId || null, autoOpen: canAutoOpen && values.autoOpen };
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
    onBlur: () => { void blur(field); }, 'aria-invalid': !!errors[field] || (field === 'port' && !!portConflictMessage),
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
    : '留空时使用项目默认端口。填写后，Bento 会尝试在启动时使用此端口。';
  const hasPortWarning = !!(portConflictMessage || checkedPort?.inUse || checkedPort?.reserved.length);
  const initialSuggestion = focusPort?.port === portNumber(values.port) ? focusPort.suggestion : null;
  const portSuggestion = (hasPortWarning ? checkedPort?.suggestion : null) ?? initialSuggestion;

  const addService = () => {
    editedServices.current = true;
    const id = 'manual:' + Date.now() + ':' + ++manualServiceIndex.current;
    setValues(current => ({
      ...current,
      services: [
        ...(current.services.length ? current.services : [{ id: id + ':root', name: current.type === 'web' ? '前端' : '主服务', dir: '.', port: current.port,
          kind: current.type === 'web' ? 'web' : undefined, key: defaultServiceKey(current.type, '.'), openWeb: true }]),
        { id, name: '', dir: '', port: '', key: '', openWeb: false },
      ],
    }));
  };
  const updateServices = (update: (services: ServiceDraft[]) => ServiceDraft[]) => {
    editedServices.current = true;
    setValues(current => {
      const services = update(current.services);
      if (services.length && !services.some(service => service.openWeb)) services[0] = { ...services[0], openWeb: true };
      return { ...current, services, port: services.length ? current.port : current.services.find(service => service.openWeb)?.port ?? current.port };
    });
  };
  const changeService = (id: string, field: ServiceField, value: string) => {
    if (field === 'key') editedServiceKeys.current.add(id);
    updateServices(services => services.map(service => {
      if (service.id !== id) return service;
      if (field === 'dir' && id.startsWith('manual:') && !editedServiceKeys.current.has(id)) {
        const key = value.trim() ? uniqueServiceKey(defaultServiceKey(service.kind, value), services.filter(row => row.id !== id)) : '';
        return { ...service, dir: value, key };
      }
      return { ...service, [field]: value };
    }));
  };
  const showGenerationPreview = () => {
    if (Object.keys(validateServices(values.services, () => undefined)).length) {
      setValidateServiceFields(true);
      toast.error('请先填写有效的服务名称、目录、角色 key 和端口');
      return;
    }
    setStandardDialog({ path: values.path, services: savedServices(values.services), mode: 'generate' });
  };

  const detectedScript = multiService ? inspection?.startScripts?.find(script => script.command === values.startCommand.trim()) : undefined;
  const serviceLayout = multiService || !!diff?.serviceChanges.length;
  const projectDetails = <div className="h1-group h1-form-group">
      <div className="space-y-2">
        <Label htmlFor="project-path">项目目录 <span className="text-destructive">*</span></Label>
        <div className="flex gap-2">
          <Input {...fieldProps('path')} placeholder="~/Projects/my-project" autoFocus={!focusPort} className="font-mono text-[length:var(--text-control)]" />
          <Button type="button" variant="outline" onClick={() => { void pickFolder(); }} disabled={saving}><FolderOpen data-icon="inline-start" />选择…</Button>
        </div>
        {message('path', '支持 ~，也可以从 Finder 拖入文件夹到侧边栏')}
        {inspection?.existingProject && <p className="text-[11px] text-warning-foreground" role="status">此目录已用于项目「{inspection.existingProject.name}」，仍可继续添加。</p>}
      </div>
      <div className="space-y-2"><Label htmlFor="project-name">名称 <span className="text-destructive">*</span></Label><Input {...fieldProps('name')} placeholder="项目名称" />{errors.name && message('name')}</div>
      <div className="space-y-2"><Label htmlFor="project-displayName">显示名称 <span className="font-normal text-muted-foreground">（可选）</span></Label><Input {...fieldProps('displayName')} placeholder="在侧边栏显示的名称" /></div>
      <div className="space-y-2">
        <Label htmlFor="project-group">分组</Label>
        <Select id="project-group" label="分组" value={groupId} options={groupOptions} onValueChange={setGroupId} disabled={saving}
          search={{ placeholder: '搜索或新建分组…', creating: groupSaving, onCreate: createGroup }} />
      </div>
      <div className="space-y-2"><Label htmlFor="project-description">描述 <span className="font-normal text-muted-foreground">（可选）</span></Label><Textarea {...fieldProps('description')} rows={2} className="resize-none" placeholder="项目用途或备注" /></div>
      {runnable && <div className="space-y-2">
        <Label id="project-type-label">类型{updated.type && <UpdatedBadge />}</Label>
        <SegmentedControl labelledBy="project-type-label" options={types} value={values.type} onChange={type => change('type', type)}
          className={cn('border', updated.type && updatedInput)} disabled={saving} />
      </div>}
  </div>;
  const runnableToggle = <div>
    <label className="flex cursor-pointer items-center gap-2.5" data-project-runnable>
      <input id="project-runnable" type="checkbox" checked={runnable} disabled={saving} className="size-3.5 shrink-0"
        aria-describedby="project-runnable-hint" onChange={event => changeRunnable(event.target.checked)} />
      <span className="shrink-0 text-[length:var(--text-control)] font-semibold">需要运行</span>
      <span id="project-runnable-hint" className="min-w-0 text-[11px] leading-relaxed text-muted-foreground">{runnable
        ? '由 Bento 启动，并显示端口与运行状态' : '无需填写端口和启动命令，终端、Git 和其他命令照常可用'}</span>
    </label>
  </div>;
  const singlePort = (
      <div className="space-y-2">
        <Label htmlFor="project-port">端口 <span className="font-normal text-muted-foreground">（可选）</span>{updated.port && <UpdatedBadge />}</Label>
        <Input {...fieldProps('port')} inputMode="numeric" className={cn('font-mono text-[length:var(--text-control)]', updated.port && updatedInput)} placeholder={values.type === 'web' ? '例如 5173' : '无开发服务器时可留空'} />
        {message('port', portHint)}
        <div id="project-port-warnings" className="space-y-2" role="status" aria-live="polite">
          {checkedPort?.inUse && <div className="flex gap-2 rounded-[9px] border border-card-border p-2.5 text-[11px] text-warning-foreground">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
            <div className="min-w-0 space-y-1">
              <p>此端口正被其他进程监听，仍可保存。</p>
              {checkedPort.inUse.owners.map((owner) => <p key={owner.pid} className="break-words font-mono">PID {owner.pid} · {owner.command || '命令不可读'}</p>)}
              {!checkedPort.inUse.owners.length && <p>无法读取监听进程的 PID / 命令。</p>}
            </div>
          </div>}
          {checkedPort?.reserved.map((warning) => <p key={warning} className="flex items-start gap-2 text-[11px] leading-relaxed text-warning-foreground"><AlertTriangle className="mt-0.5 size-3.5 shrink-0" />{warning}</p>)}
          {portCheckError && <p className="text-[11px] text-warning-foreground">{portCheckError}</p>}
          {portSuggestion != null && <Button type="button" variant="outline" size="sm" className="h-7 text-[11px]" disabled={saving} onClick={() => change('port', String(portSuggestion))}>改用 {portSuggestion}</Button>}
        </div>
        {!!inspection?.portCandidates.length && <div className="space-y-2 pt-1" role="group" aria-label="扫描到的候选端口">
          <p className="text-[11px] text-muted-foreground">扫描到的端口 · 点击填入</p>
          <div className="flex flex-wrap gap-1.5">
            {inspection.portCandidates.map((candidate) => <Button key={candidate.port} type="button" variant="outline" size="sm" disabled={saving} aria-pressed={Number(values.port) === candidate.port}
              onClick={() => change('port', String(candidate.port))}
              className={cn('h-auto max-w-full items-start gap-2 px-2.5 py-2 text-left', Number(values.port) === candidate.port && 'font-semibold')}>
              <span className="min-w-0"><span className="flex items-center gap-1.5 font-mono text-xs">{candidate.port}{Number(values.port) === candidate.port && <Check className="!size-3" />}</span><span className="mt-0.5 block whitespace-normal break-words text-[10px] font-normal leading-relaxed text-muted-foreground">{candidate.source}</span></span>
            </Button>)}
          </div>
        </div>}
        <button type="button" className="flex items-center gap-1 rounded text-[12.5px] text-foreground hover:underline" disabled={saving} onClick={addService} data-add-service>
          <Plus className="size-3.5" />添加服务
        </button>
      </div>
  );

  return (<>
    <Dialog open onOpenChange={(isOpen) => { if (!isOpen && !saving && !standardDialog) onClose(); }}>
      <DialogContent className={cn('flex max-h-[calc(100dvh-3rem)] flex-col gap-0 rounded-2xl p-0', serviceLayout ? 'max-w-[760px]' : project ? 'max-w-[680px]' : 'max-w-[620px]')}
        hideClose={saving} data-project-form onOpenAutoFocus={event => {
          if (!focusPort) return;
          const control = form.current?.querySelector<HTMLInputElement>(values.services.length
            ? '[data-service-port-input="' + CSS.escape(focusPort.serviceId) + '"]' : '#project-port');
          if (!control) return;
          event.preventDefault();
          requestAnimationFrame(() => { control.focus(); control.select(); });
        }}>
        <form ref={form} onSubmit={submit} noValidate className="flex min-h-0 flex-col overflow-hidden rounded-[inherit]"
          onCompositionStart={event => { if (event.currentTarget.contains(event.target as Node)) composing.current = true; }}
          onCompositionEnd={event => { if (event.currentTarget.contains(event.target as Node)) composing.current = false; }}
          onKeyDownCapture={(event) => {
            if (event.currentTarget.contains(event.target as Node) && event.key === 'Enter' && (composing.current || event.nativeEvent.isComposing || event.keyCode === 229)) {
              event.preventDefault();
              event.stopPropagation();
            }
          }}>
          <DialogHeader className={cn('shrink-0 gap-1 px-6 pb-4 pt-6', serviceLayout && 'px-[26px]')}>
            <div className="flex min-w-0 items-center justify-between gap-3 pr-7">
              <DialogTitle className={cn('min-w-0 truncate', serviceLayout && '!text-[17px]')} data-project-form-title>
                {project ? '编辑项目' + (serviceLayout ? ' · ' + (project.displayName || project.name) : '') : '新建项目'}
              </DialogTitle>
              {project && runnable && <Button ref={rescanButton} type="button" variant="outline" size="sm" disabled={saving || scanning} onClick={() => { void scanAgain(); }}>
                {scanning ? <Loader2 aria-hidden="true" className="!size-3.5 animate-spin" /> : <RotateCw aria-hidden="true" className="!size-3.5" />}
                {scanning ? '扫描中…' : '重新扫描'}
              </Button>}
            </div>
            <DialogDescription className={serviceLayout ? 'sr-only' : undefined}>{project ? '调整本地目录与运行配置。' : '选择本地项目目录，自动识别运行配置。'}</DialogDescription>
          </DialogHeader>
          <div className={cn('min-h-0 space-y-4 overflow-y-auto px-6 pb-5 pt-1', serviceLayout && 'space-y-[18px] px-[26px]')} data-project-form-body>
            {project && (status === 'running' || status === 'starting') && <Alert><AlertDescription>{runnable ? '项目正在运行，修改将在重启后生效' : '项目正在运行，保存后仍可在概览中停止'}</AlertDescription></Alert>}
            {saveError && <Alert variant="destructive"><AlertCircle /><AlertDescription>{saveError}</AlertDescription></Alert>}
            {(diff || appliedServiceDiff || rescanError) && <div ref={rescanCard} className="scroll-my-3" data-project-rescan-result>
              <ProjectRescanCard rows={diff?.rows} sourceFiles={diff?.sourceFiles} serviceChanges={(diff || appliedServiceDiff)?.serviceChanges}
                startScripts={(diff || appliedServiceDiff)?.startScripts} applied={!!appliedServiceDiff}
                checkStandard={showStandard} standard={standardCheck?.data} standardError={standardCheck?.error}
                onViewTemplate={showStandard && auditedServices.some(service => service.port != null)
                  ? () => setStandardDialog({ path: values.path, services: auditedServices, mode: 'view' }) : undefined}
                selected={selectedChanges} error={rescanError} disabled={saving}
                onDismiss={() => { dismissRescan(); setAppliedServiceDiff(undefined); }} onApply={applyRescan} onToggle={field => {
                  setRescan(current => {
                    if (current?.status !== 'ready') return current;
                    const unchecked = new Set(current.unchecked);
                    if (unchecked.has(field)) unchecked.delete(field); else unchecked.add(field);
                    return { ...current, unchecked };
                  });
                }} />
            </div>}
            {!serviceLayout && <section className="h1-form-section" aria-labelledby="project-details-heading">
              <h2 id="project-details-heading" className="h1-group-title">项目资料</h2>
              {projectDetails}
            </section>}
            <section className="h1-form-section" aria-labelledby="project-run-heading">
              <h2 id="project-run-heading" className="h1-group-title">运行配置</h2>
              <div className="h1-group h1-form-group">
            {runnableToggle}
            {runnable && <>
            {!multiService && singlePort}
            <div className="space-y-[7px]">
              <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <Label htmlFor="project-startCommand">{multiService ? '总启动命令' : '启动命令'} {hasServiceCommands
                  ? <span className="font-normal text-muted-foreground">（已设独立命令）</span>
                  : <span className="text-destructive">*</span>}{updated.startCommand && !detectedScript && <UpdatedBadge />}</Label>
                {project && values.services.length >= 2 && !hasServiceCommands ? <Button type="button" size="sm" variant="outline" disabled={saving}
                  className="h-7 text-xs" onClick={showGenerationPreview} data-generate-standard-script>
                  <FileCode2 data-icon="inline-start" />生成标准启动脚本
                </Button> : multiService && !hasServiceCommands && <span className="text-xs text-muted-foreground">多服务项目可用根目录脚本统一启停</span>}
              </div>
              <div className="relative">
                <Input {...fieldProps('startCommand')} className={cn('font-mono text-[length:var(--text-control)]', multiService && 'h-9 rounded-lg',
                  updated.startCommand && !multiService && updatedInput, detectedScript && 'pr-[min(55%,180px)]')} placeholder="例如 pnpm dev" />
                {detectedScript && <span data-start-script-source className="service-primary-label pointer-events-none absolute right-2.5 top-1/2 max-w-[47%] -translate-y-1/2 truncate"
                  title={detectedScript.source}>检测到 · {detectedScript.source}</span>}
              </div>
              {errors.startCommand && message('startCommand')}
              {launchPreview?.error && <p className="text-xs text-warning-foreground" role="status" data-launch-preview-error>{launchPreview.error}</p>}
              {launchPreview?.data && <div className="space-y-1.5 rounded-[9px] border border-card-border px-3 py-2.5 text-xs leading-relaxed"
                data-launch-preview aria-live="polite">
                {launchPreview.data.steps.map(step => <p key={step.serviceId ?? 'root'} className="break-words">
                  <span className="text-muted-foreground">启动时将执行：{step.dir !== '.' && step.dir + '/ · '}</span><code className="[overflow-wrap:anywhere]">{step.command}</code>
                  {step.portEnv.length > 0 && <><span className="text-muted-foreground"> · 注入 </span>
                    <code className="[overflow-wrap:anywhere]">{step.portEnv.map(({ key, value }) => key + '=' + value).join(' ')}</code></>}
                  {step.portEnv.length === 0 && <span className="text-muted-foreground"> · {step.description === '端口注入已关闭' ? '端口注入已关闭' : '无端口注入'}</span>}
                </p>)}
              </div>}
              {!!inspection?.commands.length && (!multiService || !detectedScript || inspection.commands.length > 1) &&
                <div className="flex flex-wrap gap-1.5">{inspection.commands.filter(command => command !== detectedScript?.command).slice(0, 4).map(command => <Button
                  type="button" key={command} variant="outline" size="sm" className="h-6 px-2 font-mono text-[11px]" disabled={saving}
                  onClick={() => change('startCommand', command)}>{command}</Button>)}</div>}
            </div>
            {multiService && <ProjectServiceFields services={values.services} newIds={newServiceIds} checks={portChecks} errors={serviceErrors} disabled={saving}
              portFocus={focusPort}
              onChange={changeService}
              onDefault={id => updateServices(services => services.map(service => ({ ...service, openWeb: service.id === id })))}
              onRemove={id => updateServices(services => services.filter(service => service.id !== id))} onAdd={addService} />}
            {multiService && portCheckError && <p className="text-[11px] text-warning-foreground" role="status">{portCheckError}</p>}
            <div className="space-y-[7px]">
              <Label htmlFor="project-inject-ports">端口注入</Label>
              <label className="flex min-h-9 items-center gap-2 rounded-lg border border-input px-[11px] py-2 text-[12.5px]">
                <input id="project-inject-ports" type="checkbox" role="switch" checked={values.injectPorts} disabled={saving}
                  className="size-3.5 shrink-0" onChange={event => setValues(current => ({ ...current, injectPorts: event.target.checked }))} />
                <span>{values.services.length >= 2 && !hasServiceCommands
                  ? <>向总启动命令注入 <code>PORT</code>、<code>BENTO_PORT_&lt;KEY&gt;</code> 和 <code>BENTO_PORTS</code></>
                  : '按启动方式自动适配已填写的端口'}</span>
              </label>
            </div>
            </>}
              </div>
            </section>
            {serviceLayout && <details className="h1-form-section" data-project-basic-details>
              <summary className="h1-group-title px-1.5">项目资料 <span aria-hidden="true">›</span></summary>
              {projectDetails}
            </details>}
            <ProjectAdvancedFields values={canAutoOpen ? values : { ...values, autoOpen: false }} open={advancedOpen} onOpenChange={setAdvancedOpen} disabled={saving} canAutoOpen={canAutoOpen} runnable={runnable}
              errors={advancedErrors} onBlur={() => setAdvancedErrors(validateAdvanced(values.env, values.commands))}
              onChange={patch => { setValues(current => ({ ...current, ...patch })); setAdvancedErrors({ env: [], commands: [] }); }} />
          </div>
          <footer className="h1-form-footer flex shrink-0 justify-end gap-2.5 px-6 py-3.5">
            <Button type="button" variant="outline" disabled={saving} onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || groupSaving || !!portConflictMessage || !!Object.keys(serviceConflicts).length} data-project-save>{saving && <Loader2 className="animate-spin" />}{saving ? '保存中…' : project ? '保存修改' : '添加项目'}</Button>
          </footer>
        </form>
      </DialogContent>
    </Dialog>
    <MotionPresence>{standardDialog && <ProjectStandardDialog request={standardDialog}
      onClose={() => setStandardDialog(undefined)} onGenerated={command => {
        change('startCommand', command);
        setRescan(undefined);
        setAppliedServiceDiff(undefined);
      }} />}</MotionPresence>
  </>);
}
