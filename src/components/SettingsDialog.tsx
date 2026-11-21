import type { SettingsController } from '../hooks/useSettings';
import type { UpdaterController } from '../hooks/useUpdater';
import { DEFAULT_TERMINAL_FONT_SIZE } from '../lib/terminalAppearance';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Select } from './ui/select';
import { SegmentedControl } from './ui/segmented-control';
import { Switch } from './ui/switch';

const fontSizes = Array.from({ length: 9 }, (_, index) => ({ value: String(index + 12), label: `${index + 12} px` }));
const terminalLimits = Array.from({ length: 6 }, (_, index) => ({ value: String(index + 1), label: `${index + 1} 个` }));
const appearances = [
  { value: 'system', label: '跟随系统' },
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
] as const;

export function SettingsDialog({ settings, updater, onClose, onCheckUpdates }: {
  settings: SettingsController;
  updater: UpdaterController;
  onClose: () => void;
  onCheckUpdates: () => void;
}) {
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className="flex max-h-[calc(100dvh-3rem)] max-w-lg flex-col gap-0 rounded-2xl p-0" data-settings-dialog>
      <DialogHeader className="px-6 pb-4 pt-6">
        <DialogTitle>设置</DialogTitle>
        <DialogDescription className="sr-only">管理外观、登录时启动和终端偏好。</DialogDescription>
      </DialogHeader>

      <div className="min-h-0 space-y-4 overflow-y-auto px-6 pb-6">
      <section className="h1-form-section" aria-labelledby="settings-general">
        <h3 id="settings-general" className="h1-group-title">通用</h3>
        <div className="h1-group">
        <div className="h1-list-row">
          <span id="settings-appearance" className="text-sm font-medium">外观</span>
          <SegmentedControl labelledBy="settings-appearance" value={settings.appearance} options={appearances}
            disabled={settings.fontLoading || settings.appearanceSaving}
            className="shrink-0 [&>button]:flex-none [&>button]:whitespace-nowrap [&>button]:px-3"
            onChange={value => { void settings.changeAppearance(value); }} />
        </div>
        {settings.appearanceError && <p role="alert" className="h1-list-note">{settings.appearanceError}</p>}
        <div className="h1-list-row">
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
        {settings.autostartError && <p role="alert" className="h1-list-note" data-autostart-error>{settings.autostartError}</p>}
        <div className="h1-list-row">
          <label htmlFor="settings-show-tray" className="text-sm font-medium">在菜单栏显示</label>
          <Switch id="settings-show-tray" checked={settings.features['tray.enabled']}
            disabled={settings.fontLoading || !!settings.featureSaving}
            onCheckedChange={value => { void settings.changeFeature('tray.enabled', value); }} />
        </div>
        </div>
      </section>

      <section className="h1-form-section" aria-labelledby="settings-terminal">
        <h3 id="settings-terminal" className="h1-group-title">终端</h3>
        <div className="h1-group">
        <div className="h1-list-row">
          <div>
            <label htmlFor="settings-font-size" className="text-sm font-medium">字号</label>
            <p id="settings-font-help" className="mt-1.5 text-xs text-muted-foreground">立即应用到所有终端，默认 {DEFAULT_TERMINAL_FONT_SIZE} px</p>
          </div>
          <Select id="settings-font-size" label="字号" value={String(settings.fontSize)} options={fontSizes} clearRowsWhenAbove
            disabled={settings.fontLoading || settings.fontSaving} aria-describedby="settings-font-help" data-settings-font-size
            onValueChange={value => { void settings.changeFontSize(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
        </div>
        {settings.fontError && <p role="alert" className="h1-list-note" data-font-size-error>{settings.fontError}</p>}
        <div className="h1-list-row">
          <div>
            <label htmlFor="settings-max-terminals" className="text-sm font-medium">每个项目最多终端数</label>
            <p id="settings-max-terminals-help" className="mt-1.5 text-xs leading-relaxed text-muted-foreground">默认 2 个；调低不会关闭已打开的终端</p>
          </div>
          <Select id="settings-max-terminals" label="每个项目最多终端数" value={String(settings.maxPerProject)} options={terminalLimits} clearRowsWhenAbove
            disabled={settings.fontLoading || settings.maxSaving} aria-describedby="settings-max-terminals-help" data-settings-max-terminals
            onValueChange={value => { void settings.changeMaxPerProject(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
        </div>
        {settings.maxError && <p role="alert" className="h1-list-note" data-max-terminals-error>{settings.maxError}</p>}
        {([
          ['terminal.restore', '重启后恢复终端'],
          ['terminal.commandNotices', '命令完成通知'],
        ] as const).map(([key, label]) => <div key={key} className="h1-list-row">
          <label htmlFor={key} className="text-sm font-medium">{label}</label>
          <Switch id={key} checked={settings.features[key]} disabled={settings.fontLoading || !!settings.featureSaving}
            onCheckedChange={value => { void settings.changeFeature(key, value); }} />
        </div>)}
        {settings.featureError && <p role="alert" className="h1-list-note">{settings.featureError}</p>}
        </div>
      </section>

      <section className="h1-form-section" aria-labelledby="settings-about">
        <h3 id="settings-about" className="h1-group-title">关于</h3>
        <div className="h1-group">
        <div className="h1-list-row">
          <p className="text-sm text-muted-foreground">版本 <span className="tabular-nums text-foreground">v{updater.currentVersion || '…'}</span></p>
          <Button variant="outline" size="sm" onClick={onCheckUpdates}>检查更新</Button>
        </div>
        </div>
      </section>
      </div>
    </DialogContent>
  </Dialog>;
}
