import { useState } from 'react';
import type { ReactNode } from 'react';
import { Play, Folder, Terminal, AlertCircle, Loader2, Check, Copy, Globe, AppWindow, Pencil, Trash2, GitBranch } from 'lucide-react';
import { toast } from 'sonner';
import type { ProjectConfig, ProjectStatus, ProjectRuntimeStatus, PortConflict, GitInfo } from '../types';
import { useGitInfo } from '../lib/useGitInfo';
import { useHasProjectLogs } from '../lib/useProjectLogs';
import { cn } from '../lib/utils';
import { ProjectAvatar } from './ProjectAvatar';
import Button from './ui/button';
import { ActionTooltip } from './ui/tooltip';
import { Badge } from './ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from './ui/card';
import { Alert, AlertDescription, AlertTitle } from './ui/alert';

interface ProjectCardProps {
  project: ProjectConfig;
  status: ProjectStatus;
  runtime?: ProjectRuntimeStatus;
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

function ProjectMetric({ label, children }: { label: string; children: ReactNode }) {
  return <Card className="min-w-0 px-4 py-3.5">
    <dt className="text-meta leading-4 text-muted-foreground">{label}</dt>
    <dd className="mt-1.5 min-w-0 break-words font-mono text-base font-semibold leading-[22px] select-text">{children}</dd>
  </Card>;
}

function GitSummary({ git }: { git: GitInfo }) {
  const divergence = [git.ahead && `↑${git.ahead}`, git.behind && `↓${git.behind}`].filter(Boolean).join(' ');
  const divergenceLabel = [git.ahead && `领先 ${git.ahead} 个提交`, git.behind && `落后 ${git.behind} 个提交`].filter(Boolean).join('，');
  const branch = git.branch ?? `分离的 HEAD · ${git.head ?? '未知'}`;
  return <>
    <ProjectMetric label="分支">
      <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
        <span className="flex min-w-0 items-center gap-1.5">
          <GitBranch aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
          <code className="truncate" title={branch}>{branch}</code>
        </span>
        {git.initial && <span className="font-sans text-label font-normal text-muted-foreground">尚无提交</span>}
        {divergence && <ActionTooltip label={`相对 ${git.upstream ?? '上游分支'}：${divergenceLabel}`}>
          <span className="text-label font-normal tabular-nums text-muted-foreground" data-git-divergence>{divergence}</span>
        </ActionTooltip>}
      </span>
    </ProjectMetric>
    <ProjectMetric label="未提交改动">
      <span className={cn(git.changes > 0 && 'text-warning-foreground')} data-git-changes={git.changes}>{git.changes}</span>
    </ProjectMetric>
  </>;
}

export function ProjectCard({ project, status, runtime, conflict, onStart, onStop, onRestart, onOpenBrowser, onRevealPath, onOpenLogs, onEdit, onDelete, onRunCommand }: ProjectCardProps) {
  const hasLogs = useHasProjectLogs(project.id);
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
    <div className="flex min-w-0 flex-col gap-3.5" data-project-details={project.id} data-project-runtime={status}>
      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-3 border-b border-[#f2f2f4] pb-5">
        <div className="flex min-w-0 flex-1 basis-60 items-center gap-[13px]">
          <ProjectAvatar project={project} status={status} conflict={!!conflict} className="size-10 rounded-[10px] text-[17px]" />
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
              <h1 className="min-w-0 break-words text-xl font-heading tracking-[-.015em] select-text">{project.displayName || project.name}</h1>
              <Badge className={cn('rounded-full px-2.5 py-[3px] text-meta', isRunning && !conflict && 'border-0 bg-[#ecfdf5] text-[#047857]')} variant={conflict ? 'warning' : isRunning ? 'success' : status === 'error' ? 'destructive' : 'outline'}>
                {isStarting ? <Loader2 className="size-3 animate-spin" /> : <span className={cn('size-1.5 rounded-full', isRunning && !conflict ? 'bg-[#10b981]' : 'bg-current opacity-70')} />}
                {conflict ? '端口冲突' : statusNames[status]}
                {isRunning && !conflict && project.port != null && <span>· {project.port}</span>}
              </Badge>
              {runtime?.external && <Badge variant="outline" data-external-badge>外部进程运行</Badge>}
            </div>
            <code className="mt-[3px] block truncate text-xs leading-[17px] text-muted-foreground" title={project.path}>{project.path}</code>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {isRunning && !isApp && project.port && <Button size="sm" variant="outline" onClick={() => onOpenBrowser(project.port!)} data-project-action="open">打开网页</Button>}
          {status === 'stopped' || status === 'error' ? (
            <Button size="sm" disabled={!!conflict} onClick={() => onStart(project.id)} data-project-action="start"><Play data-icon="inline-start" />启动</Button>
          ) : <>
            <Button size="sm" variant="outline" disabled={isStarting} onClick={() => onRestart(project.id)} data-project-action="restart">重启</Button>
            <Button size="sm" onClick={() => onStop(project.id)} data-project-action="stop">停止</Button>
          </>}
        </div>
      </header>

      {conflict && <Alert variant="warning">
        <AlertCircle /><AlertTitle>端口 <code>:{project.port}</code> 存在冲突</AlertTitle>
        <AlertDescription>与 {conflict.projectNames.filter((name) => name !== (project.displayName || project.name)).join('、')} 使用相同端口，已限制启动。请为其中一个项目更换端口。</AlertDescription>
      </Alert>}

      {status === 'error' && <Alert variant="destructive" data-project-error>
        <AlertCircle /><AlertTitle>进程异常退出（{runtime?.exitSignal != null ? `信号 ${runtime.exitSignal}` : runtime?.exitCode != null ? `状态码 ${runtime.exitCode}` : '未知状态'}）</AlertTitle>
        <AlertDescription><Button variant="outline" size="sm" onClick={onOpenLogs}>查看日志</Button></AlertDescription>
      </Alert>}

      <dl className={cn('mt-2 grid grid-cols-2 gap-3', git && 'lg:grid-cols-4')} data-git-summary={git ? '' : undefined}>
        {git && <GitSummary git={git} />}
        <ProjectMetric label="启动命令"><code title={project.startCommand}>{project.startCommand}</code></ProjectMetric>
        <ProjectMetric label="配置端口">{project.port ?? <span className="font-sans text-sm font-normal text-muted-foreground">未设置</span>}</ProjectMetric>
      </dl>

      <Card>
        <CardHeader className="flex-row items-center justify-between gap-3 p-[18px]">
          <div className="flex min-w-0 flex-col gap-1.5">
            <CardTitle className="text-[13.5px]">项目配置</CardTitle>
            <CardDescription className="flex flex-wrap items-center gap-x-1.5 gap-y-1 break-words">
              <TypeIcon aria-hidden="true" className="size-3.5 shrink-0" />
              <span>{isApp ? '桌面应用' : 'Web 项目'}</span>
              {project.displayName && <span className="min-w-0">· {project.name}</span>}
              {project.autoOpen && !isApp && <span data-auto-open-note>· 启动就绪后自动打开网页</span>}
            </CardDescription>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <ActionTooltip label="编辑项目"><Button variant="outline" size="sm" onClick={onEdit} data-edit-project><Pencil data-icon="inline-start" />编辑</Button></ActionTooltip>
            <ActionTooltip label="删除项目"><Button variant="ghost-destructive" size="icon" aria-label="删除项目" onClick={onDelete} data-delete-project><Trash2 /></Button></ActionTooltip>
          </div>
        </CardHeader>
        {(project.description || !!project.env?.length || !!project.commands?.length) && <CardContent className="flex flex-col gap-3 px-[18px] pb-4">
          {project.description && <p className="max-w-2xl break-words text-sm text-[#6e6e73] select-text">{project.description}</p>}
          {(!!project.env?.length || !!project.commands?.length) && <dl className="flex min-w-0 flex-col gap-3 text-xs">
            {!!project.env?.length && <div className="flex items-start gap-4" data-env-summary>
              <dt className="w-16 shrink-0 text-muted-foreground">环境变量</dt>
              <dd className="min-w-0 flex-1 break-words select-text">{project.env.map(entry => entry.key).join('、')}<span className="ml-2 text-muted-foreground">（{project.env.length} 个）</span></dd>
            </div>}
            {!!project.commands?.length && <div className="flex items-center gap-4">
              <dt className="w-16 shrink-0 text-muted-foreground">其他命令</dt>
              <dd className="flex min-w-0 flex-1 flex-wrap gap-2">{project.commands.map(entry =>
                <ActionTooltip key={entry.name} label={entry.command}><Button size="sm" variant="outline" onClick={() => onRunCommand(entry.command)} title={entry.command} data-run-command={entry.name}><Play data-icon="inline-start" />运行 {entry.name}</Button></ActionTooltip>
              )}</dd>
            </div>}
          </dl>}
        </CardContent>}
        <CardFooter className="flex-wrap gap-2 px-[18px] pb-[18px]">
          <ActionTooltip label="在 Finder 中定位">
            <Button variant="outline" size="sm" onClick={() => onRevealPath(project.path)} data-reveal-path><Folder data-icon="inline-start" />Finder</Button>
          </ActionTooltip>
          <ActionTooltip label={copiedPath ? '已复制' : '复制路径'}>
            <Button variant="ghost" size="icon-sm" aria-label="复制路径" onClick={handleCopyPath} data-copy-path>{copiedPath ? <Check /> : <Copy />}</Button>
          </ActionTooltip>
          <Button size="sm" variant="outline" onClick={onOpenLogs} className="ml-auto" data-open-logs><Terminal data-icon="inline-start" />终端日志{hasLogs && <span className="size-1.5 rounded-full bg-success" />}</Button>
        </CardFooter>
      </Card>
    </div>
  );
}
