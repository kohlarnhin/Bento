import { ArrowDownToLine, ArrowRight, Loader2, Settings } from 'lucide-react';
import type { UpdaterController } from '../hooks/useUpdater';
import type { ProjectConfig } from '../types';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Alert, AlertDescription } from './ui/alert';
import { ActionTooltip } from './ui/tooltip';
import { ScrollArea } from './ui/scroll-area';

export function UpdateFooter({ updater, onOpenSettings }: { updater: UpdaterController; onOpenSettings: () => void }) {
  return <footer className="flex shrink-0 items-center justify-between gap-2 border-t border-sidebar-border px-3 py-2" data-update-footer>
    <ActionTooltip label="检查更新" side="right">
      <Button variant={updater.update ? 'secondary' : 'ghost'} size="sm" onClick={() => { void updater.checkNow(); }} data-check-updates>
        {updater.update ? <><ArrowDownToLine data-icon="inline-start" />更新到 v{updater.update.version}</> : updater.currentVersion ? `v${updater.currentVersion}` : '检查更新'}
      </Button>
    </ActionTooltip>
    <ActionTooltip label="设置" side="right">
      <Button variant="ghost" size="icon" aria-label="设置" onClick={onOpenSettings} data-open-settings><Settings /></Button>
    </ActionTooltip>
  </footer>;
}

export function UpdateDialog({ updater, projects, terminalCount }: {
  updater: UpdaterController; projects: ProjectConfig[]; terminalCount: number;
}) {
  const { status, update, currentVersion, isBusy } = updater;
  const available = status === 'available' && update;
  const date = update?.date ? new Date(update.date) : null;
  const title = available ? `Bento v${update.version} 可用` : status === 'up-to-date' ? `已是最新版本（v${currentVersion}）` : status === 'error' ? '更新未完成' : isBusy ? '正在更新 Bento' : '检查更新';
  const progressText = status === 'installing' ? '正在安装…' : status === 'restarting' ? '正在重启…' : '正在下载并验证签名…';
  return <Dialog open={updater.isModalOpen} onOpenChange={open => { if (!open) updater.closeModal(); }}>
    <DialogContent hideClose={isBusy} data-update-dialog data-update-state={status}
      onEscapeKeyDown={event => { if (isBusy) event.preventDefault(); }}
      onInteractOutside={event => { if (isBusy) event.preventDefault(); }}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>
          {available ? <span className="inline-flex items-center gap-2">v{currentVersion}<ArrowRight className="size-3" />v{update.version}</span>
            : status === 'checking' ? '正在查找新版本，请稍候。'
              : isBusy ? '完成后会停止 Bento 管理的任务并重新打开应用。'
                : status === 'up-to-date' ? '当前已安装最新版本。' : '你可以稍后再试。'}
        </DialogDescription>
      </DialogHeader>

      {available && <>
        {date && !Number.isNaN(date.getTime()) && <p className="text-xs text-muted-foreground">发布于 {date.toLocaleDateString('zh-CN')}</p>}
        <ScrollArea className="max-h-52 [&_[data-radix-scroll-area-viewport]]:max-h-52">
          <p className="whitespace-pre-wrap break-words pr-3 text-sm leading-relaxed select-text" data-update-notes>{update.body || '此版本暂无更新说明。'}</p>
        </ScrollArea>
        {(projects.length > 0 || terminalCount > 0) && <Alert>
          <AlertDescription>
            <div className="flex flex-col gap-2" data-update-shutdown>
              {projects.length > 0 && <p>正在运行的项目：{projects.map(project => project.displayName || project.name).join('、')}（将被停止）</p>}
              {terminalCount > 0 && <p>打开的终端：{terminalCount} 个（其中运行的命令会被终止）</p>}
            </div>
          </AlertDescription>
        </Alert>}
      </>}

      {status === 'checking' && <div className="flex items-center gap-2 py-4 text-sm text-muted-foreground" role="status"><Loader2 className="size-4 animate-spin" />正在检查…</div>}
      {isBusy && <div className="flex flex-col gap-3 py-2" role="status" aria-live="polite">
        <div className="flex justify-between gap-3 text-sm"><span>{progressText}</span><span className="tabular-nums">{updater.downloadProgress === null ? '' : `${updater.downloadProgress} %`}</span></div>
        <progress aria-label="更新下载进度" max={100} value={updater.downloadProgress ?? undefined} className="h-1.5 w-full overflow-hidden rounded-full appearance-none [&::-webkit-progress-bar]:bg-secondary [&::-webkit-progress-value]:bg-primary" />
      </div>}
      {status === 'error' && <Alert variant="destructive"><AlertDescription className="break-words select-text" data-update-error>{updater.error}</AlertDescription></Alert>}

      <div className="flex justify-end gap-2 pt-1">
        {!isBusy && <Button variant="outline" onClick={updater.closeModal}>{available ? '稍后' : '关闭'}</Button>}
        {available && <Button onClick={() => { void updater.installUpdate(); }} data-install-update><ArrowDownToLine data-icon="inline-start" />更新并重启</Button>}
        {status === 'error' && <Button onClick={() => { void updater.retry(); }} data-retry-update>重试</Button>}
        {isBusy && <Button disabled><Loader2 data-icon="inline-start" className="animate-spin" />{status === 'downloading' ? '下载中' : status === 'installing' ? '安装中' : '重启中'}</Button>}
      </div>
    </DialogContent>
  </Dialog>;
}
