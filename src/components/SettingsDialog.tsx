import type { SettingsController } from '../hooks/useSettings';
import type { UpdaterController } from '../hooks/useUpdater';
import { DEFAULT_TERMINAL_FONT_SIZE } from '../lib/terminalAppearance';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Select } from './ui/select';
import { Switch } from './ui/switch';

const fontSizes = Array.from({ length: 9 }, (_, index) => ({ value: String(index + 12), label: `${index + 12} px` }));
const terminalLimits = Array.from({ length: 6 }, (_, index) => ({ value: String(index + 1), label: `${index + 1} 个` }));

export function SettingsDialog({ settings, updater, onClose, onCheckUpdates }: {
  settings: SettingsController;
  updater: UpdaterController;
  onClose: () => void;
  onCheckUpdates: () => void;
}) {
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className="flex max-h-[calc(100dvh-3rem)] max-w-lg flex-col gap-0 rounded-2xl p-0" data-settings-dialog>
      <DialogHeader className="px-6 pb-5 pt-6">
        <DialogTitle>设置</DialogTitle>
        <DialogDescription>管理登录时启动和终端偏好。</DialogDescription>
      </DialogHeader>

      <div className="min-h-0 overflow-y-auto rounded-b-2xl">
      <section className="border-t px-6 py-5" aria-labelledby="settings-general">
        <h3 id="settings-general" className="mb-4 text-xs font-medium text-muted-foreground">通用</h3>
        <div className="flex items-start justify-between gap-5">
          <div className="min-w-0">
            <label htmlFor="settings-autostart" className="text-sm font-medium">登录时启动 Bento</label>
            <p id="settings-autostart-help" className="mt-1.5 max-w-72 text-xs leading-relaxed text-muted-foreground">登录 macOS 后在菜单栏中静默运行，不打开窗口</p>
            {settings.autostart?.reason && <p id="settings-autostart-reason" className="mt-2 text-xs text-muted-foreground">{settings.autostart.reason}</p>}
          </div>
          <Switch id="settings-autostart" className="mt-0.5" checked={settings.autostart?.enabled ?? false}
            disabled={!settings.autostart?.available || settings.autostartSaving}
            aria-describedby={'settings-autostart-help' + (settings.autostart?.reason ? ' settings-autostart-reason' : '')}
            onCheckedChange={enabled => { void settings.changeAutostart(enabled); }} />
        </div>
        {settings.autostartError && <p role="alert" className="mt-3 break-words text-xs leading-relaxed text-destructive" data-autostart-error>{settings.autostartError}</p>}
      </section>

      <section className="border-t px-6 py-5" aria-labelledby="settings-terminal">
        <h3 id="settings-terminal" className="mb-4 text-xs font-medium text-muted-foreground">终端</h3>
        <div className="flex items-center justify-between gap-5">
          <div>
            <label htmlFor="settings-font-size" className="text-sm font-medium">字号</label>
            <p id="settings-font-help" className="mt-1.5 text-xs text-muted-foreground">立即应用到所有终端，默认 {DEFAULT_TERMINAL_FONT_SIZE} px</p>
          </div>
          <Select id="settings-font-size" label="字号" value={String(settings.fontSize)} options={fontSizes}
            disabled={settings.fontLoading || settings.fontSaving} aria-describedby="settings-font-help" data-settings-font-size
            onValueChange={value => { void settings.changeFontSize(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
        </div>
        {settings.fontError && <p role="alert" className="mt-3 break-words text-xs leading-relaxed text-destructive" data-font-size-error>{settings.fontError}</p>}
        <div className="mt-5 flex items-center justify-between gap-5">
          <div>
            <label htmlFor="settings-max-terminals" className="text-sm font-medium">每个项目最多终端数</label>
            <p id="settings-max-terminals-help" className="mt-1.5 text-xs leading-relaxed text-muted-foreground">默认 2 个；调低不会关闭已打开的终端</p>
          </div>
          <Select id="settings-max-terminals" label="每个项目最多终端数" value={String(settings.maxPerProject)} options={terminalLimits}
            disabled={settings.fontLoading || settings.maxSaving} aria-describedby="settings-max-terminals-help" data-settings-max-terminals
            onValueChange={value => { void settings.changeMaxPerProject(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
        </div>
        {settings.maxError && <p role="alert" className="mt-3 break-words text-xs leading-relaxed text-destructive" data-max-terminals-error>{settings.maxError}</p>}
      </section>

      <section className="border-t px-6 py-5" aria-labelledby="settings-about">
        <h3 id="settings-about" className="mb-4 text-xs font-medium text-muted-foreground">关于</h3>
        <div className="flex items-center justify-between gap-5">
          <p className="text-sm text-muted-foreground">版本 <span className="tabular-nums text-foreground">v{updater.currentVersion || '…'}</span></p>
          <Button variant="outline" size="sm" onClick={onCheckUpdates}>检查更新</Button>
        </div>
      </section>
      </div>
    </DialogContent>
  </Dialog>;
}
