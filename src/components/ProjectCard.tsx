import { useState } from 'react';
import { Play, Square, RefreshCw, ExternalLink, Folder, Terminal, AlertCircle, Loader2, Check, Copy, Globe, AppWindow, Pencil, Trash2, GitBranch } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig, ProjectStatus, ProjectRuntimeStatus, LogEntry, PortConflict, GitInfo } from '../types';
import { useGitInfo } from '../lib/useGitInfo';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';
import { Badge } from './ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from './ui/card';
import { Alert, AlertDescription, AlertTitle } from './ui/alert';
import { Separator } from './ui/separator';

interface ProjectCardProps {
  project: ProjectConfig;
  status: ProjectStatus;
  runtime?: ProjectRuntimeStatus;
  logs: LogEntry[];
  conflict?: PortConflict;
  onStart: (id: string) => void;
  onStop: (id: string) => void;
  onRestart: (id: string) => void;
  onOpenBrowser: (port: number) => void;
  onRevealPath: (path: string) => void;
  onOpenLogs: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onRunCommand: (command: string) => void;
}

const statusNames: Record<ProjectStatus, string> = { running: '运行中', starting: '启动中', stopped: '未运行', error: '运行异常' };

function GitSummary({ git }: { git: GitInfo }) {
  const divergence = [git.ahead && `↑${git.ahead}`, git.behind && `↓${git.behind}`].filter(Boolean).join(' ');
  const divergenceLabel = [git.ahead && `领先 ${git.ahead} 个提交`, git.behind && `落后 ${git.behind} 个提交`].filter(Boolean).join('，');
  return (
    <dd className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1 select-text" data-git-summary>
      <span className="flex min-w-0 items-center gap-2">
        <GitBranch className="size-4 shrink-0 text-muted-foreground" />
        <code className="truncate text-xs" title={git.branch ?? undefined}>{git.branch ?? `分离的 HEAD · ${git.head ?? '未知'}`}</code>
        {git.initial && <span className="text-[11px] text-muted-foreground">尚无提交</span>}
      </span>
      {git.changes > 0
        ? <span className="text-xs text-warning-foreground" data-git-changes={git.changes}>{git.changes} 个未提交改动</span>
        : <span className="text-xs text-muted-foreground" data-git-changes={0}>无未提交改动</span>}
      {divergence && <ActionTooltip label={`相对 ${git.upstream ?? '上游分支'}：${divergenceLabel}`}>
        <span className="text-xs tabular-nums text-muted-foreground" data-git-divergence>{divergence}</span>
      </ActionTooltip>}
    </dd>
  );
}

export function ProjectCard({ project, status, runtime, logs, conflict, onStart, onStop, onRestart, onOpenBrowser, onRevealPath, onOpenLogs, onEdit, onDelete, onRunCommand }: ProjectCardProps) {
  const [copiedPath, setCopiedPath] = useState(false);
  const git = useGitInfo(project.id, project.path);
  const isRunning = status === 'running';
  const isStarting = status === 'starting';
  const isApp = project.type === 'app';
  const TypeIcon = isApp ? AppWindow : Globe;
  const handleCopyPath = async () => {
    try {
      await navigator.clipboard.writeText(project.path);
      setCopiedPath(true);
      toast.success('项目路径已复制');
      setTimeout(() => setCopiedPath(false), 2000);
    } catch (error) {
      toast.error(`复制路径失败：${error}`);
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-7" data-project-details={project.id} data-project-runtime={status}>
      <header className="flex flex-col gap-3">
        <div className="flex items-center gap-2 text-xs text-muted-foreground"><TypeIcon className="size-4" /><span>{isApp ? '桌面应用' : 'Web 项目'}</span></div>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="min-w-0 break-words text-2xl font-semibold tracking-tight select-text lg:text-3xl">{project.displayName || project.name}</h1>
          <Badge variant="secondary">{isApp ? 'App' : 'Web'}</Badge>
          <Badge variant={conflict ? 'warning' : isRunning ? 'success' : status === 'error' ? 'destructive' : 'outline'}>
            {isStarting ? <Loader2 className="size-3 animate-spin" /> : <span className="size-1.5 rounded-full bg-current opacity-70" />}
            {conflict ? '端口冲突' : statusNames[status]}
          </Badge>
          {runtime?.external && <Badge variant="outline" data-external-badge>外部进程运行</Badge>}
        </div>
        {project.description && <p className="max-w-2xl break-words text-sm leading-relaxed text-muted-foreground select-text">{project.description}</p>}
      </header>

      {conflict && <Alert variant="warning">
        <AlertCircle /><AlertTitle>端口 :{project.port} 存在冲突</AlertTitle>
        <AlertDescription>与 {conflict.projectNames.filter((name) => name !== (project.displayName || project.name)).join('、')} 使用相同端口，已限制启动。请为其中一个项目更换端口。</AlertDescription>
      </Alert>}

      {status === 'error' && <Alert variant="destructive" data-project-error>
        <AlertCircle /><AlertTitle>进程异常退出（{runtime?.exitSignal != null ? `信号 ${runtime.exitSignal}` : runtime?.exitCode != null ? `状态码 ${runtime.exitCode}` : '未知状态'}）</AlertTitle>
        <AlertDescription><Button variant="outline" size="sm" onClick={onOpenLogs}>查看日志</Button></AlertDescription>
      </Alert>}

      <Card>
        <CardHeader className="flex-row items-center justify-between gap-3 space-y-0">
          <div className="min-w-0 space-y-1.5">
            <CardTitle>项目配置</CardTitle>
            <CardDescription className="break-words">{project.displayName ? project.name : '本地目录与运行配置'}</CardDescription>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <ActionTooltip label="编辑项目"><Button variant="outline" size="sm" onClick={onEdit} data-edit-project><Pencil />编辑</Button></ActionTooltip>
            <ActionTooltip label="删除项目"><Button variant="ghost-destructive" size="icon" aria-label="删除项目" onClick={onDelete} data-delete-project><Trash2 /></Button></ActionTooltip>
          </div>
        </CardHeader>
        <CardContent>
          <dl className="flex min-w-0 flex-col text-[13px]">
            <div className="flex flex-wrap items-center gap-x-5 gap-y-2 pb-4">
              <dt className="w-20 shrink-0 text-muted-foreground">项目路径</dt>
              <dd className="flex min-w-0 flex-1 items-center gap-2">
                <Folder className="size-4 shrink-0 text-muted-foreground" />
                <code className="min-w-0 flex-1 truncate text-xs" title={project.path}>{project.path}</code>
                <ActionTooltip label={copiedPath ? '已复制' : '复制路径'}>
                  <Button variant="ghost" size="icon-sm" aria-label="复制路径" onClick={handleCopyPath} data-copy-path>{copiedPath ? <Check /> : <Copy />}</Button>
                </ActionTooltip>
                <ActionTooltip label="在 Finder 中定位">
                  <Button variant="outline" size="sm" onClick={() => onRevealPath(project.path)} data-reveal-path>Finder<ExternalLink data-icon="inline-end" /></Button>
                </ActionTooltip>
              </dd>
            </div>
            {git && <><Separator /><div className="flex items-center gap-5 py-4">
              <dt className="w-20 shrink-0 text-muted-foreground">Git</dt>
              <GitSummary git={git} />
            </div></>}
            <Separator />
            <div className="flex items-center gap-5 py-4">
              <dt className="w-20 shrink-0 text-muted-foreground">{isApp ? '应用类型' : '配置端口'}</dt>
              <dd className="flex flex-wrap items-center gap-2 select-text">{isApp ? `macOS 桌面应用${project.port ? ` · :${project.port}` : ''}` : project.port ? <code className="text-xs">:{project.port}</code> : <span className="text-muted-foreground">未设置</span>}{project.autoOpen && !isApp && <span className="text-[11px] text-muted-foreground" data-auto-open-note>启动就绪后自动打开网页</span>}</dd>
            </div>
            <Separator />
            <div className="flex items-start gap-5 pt-4">
              <dt className="w-20 shrink-0 pt-1.5 text-muted-foreground">启动命令</dt>
              <dd className="min-w-0 flex-1 rounded-md border bg-muted/40 px-3 py-2"><code className="block break-all text-xs leading-relaxed">{project.startCommand}</code></dd>
            </div>
            {!!project.env?.length && <><Separator className="mt-4" /><div className="flex items-start gap-5 pt-4" data-env-summary><dt className="w-20 shrink-0 text-muted-foreground">环境变量</dt><dd className="min-w-0 flex-1 break-words text-xs leading-relaxed select-text">{project.env.map(entry => entry.key).join('、')}<span className="ml-2 text-muted-foreground">（{project.env.length} 个）</span></dd></div></>}
            {!!project.commands?.length && <><Separator className="mt-4" /><div className="flex items-start gap-5 pt-4"><dt className="w-20 shrink-0 pt-1.5 text-muted-foreground">其他命令</dt><dd className="flex min-w-0 flex-1 flex-wrap gap-2">{project.commands.map(entry => <ActionTooltip key={entry.name} label={entry.command}><Button size="sm" variant="outline" onClick={() => onRunCommand(entry.command)} title={entry.command} data-run-command={entry.name}><Play />运行 {entry.name}</Button></ActionTooltip>)}</dd></div></>}
          </dl>
        </CardContent>
        <CardFooter className="flex-wrap gap-2">
          {status === 'stopped' || status === 'error' ? (
            <Button size="sm" disabled={!!conflict || isStarting} onClick={() => onStart(project.id)} data-project-action="start"><Play data-icon="inline-start" />启动</Button>
          ) : <>
            <Button size="sm" variant="destructive" onClick={() => onStop(project.id)} data-project-action="stop"><Square data-icon="inline-start" />停止</Button>
            <Button size="sm" variant="outline" disabled={isStarting} onClick={() => onRestart(project.id)} data-project-action="restart"><RefreshCw data-icon="inline-start" />重启</Button>
          </>}
          {isRunning && !isApp && project.port && <Button size="sm" variant="outline" onClick={() => onOpenBrowser(project.port!)} data-project-action="open"><ExternalLink data-icon="inline-start" />打开网页</Button>}
          <Button size="sm" variant="outline" onClick={onOpenLogs} className="ml-auto" data-open-logs><Terminal data-icon="inline-start" />终端日志{logs.length > 0 && <span className="size-1.5 rounded-full bg-success" />}</Button>
        </CardFooter>
      </Card>
    </div>
  );
}
