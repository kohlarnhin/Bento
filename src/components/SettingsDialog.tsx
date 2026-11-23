import type { KeyboardEvent, ReactNode } from 'react';
import { Bell, Info, SlidersHorizontal, SquareTerminal } from 'lucide-react';
import type { SettingsController } from '../hooks/useSettings';
import type { UpdaterController } from '../hooks/useUpdater';
import { DEFAULT_TERMINAL_FONT_SIZE } from '../lib/terminalAppearance';
import Button from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './ui/dialog';
import { Select } from './ui/select';
import { SegmentedControl } from './ui/segmented-control';
import { Switch } from './ui/switch';

const categories = [
  { id: 'general', label: '通用', icon: SlidersHorizontal },
  { id: 'terminal', label: '终端', icon: SquareTerminal },
  { id: 'notifications', label: '通知', icon: Bell },
  { id: 'about', label: '关于', icon: Info },
] as const;

export type SettingsCategory = typeof categories[number]['id'];

const fontSizes = Array.from({ length: 9 }, (_, index) => ({ value: String(index + 12), label: `${index + 12} px` }));
const terminalLimits = Array.from({ length: 6 }, (_, index) => ({ value: String(index + 1), label: `${index + 1} 个` }));
const appearances = [
  { value: 'system', label: '跟随系统' },
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
] as const;

function SettingsGroup({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return <section className="h1-form-section" aria-labelledby={id}>
    <h3 id={id} className="h1-group-title">{title}</h3>
    <div className="settings-group">{children}</div>
  </section>;
}

function SettingsRow({ label, description, error, children }: {
  label: ReactNode;
  description?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
}) {
  return <div className="settings-row">
    <div className="h1-list-row max-[480px]:flex-wrap max-[480px]:gap-2">
      <div className="min-w-0">
        <div className="text-[13.5px] leading-[18px]">{label}</div>
        {description && <div className="mt-1 space-y-1 text-xs leading-4 text-muted-foreground">{description}</div>}
      </div>
      {children}
    </div>
    {error}
  </div>;
}

function FeatureRow({ settings, feature, id = feature, label, description }: {
  settings: SettingsController;
  feature: keyof SettingsController['features'];
  id?: string;
  label: string;
  description?: string;
}) {
  const descriptionId = description ? `${id}-help` : undefined;
  const error = settings.featureError?.key === feature ? settings.featureError.message : null;
  return <SettingsRow label={<label htmlFor={id}>{label}</label>}
    description={description && <p id={descriptionId}>{description}</p>}
    error={error && <p role="alert" className="h1-list-note">{error}</p>}>
    <Switch id={id} checked={settings.features[feature]} disabled={settings.fontLoading || !!settings.featureSaving}
      aria-describedby={descriptionId} onCheckedChange={value => { void settings.changeFeature(feature, value); }} />
  </SettingsRow>;
}

function GeneralSettings({ settings }: { settings: SettingsController }) {
  const autostartReason = settings.autostart?.reason;
  return <>
    <SettingsGroup id="settings-appearance-group" title="外观">
      <SettingsRow label={<span id="settings-appearance">主题</span>}
        error={settings.appearanceError && <p role="alert" className="h1-list-note">{settings.appearanceError}</p>}>
        <SegmentedControl labelledBy="settings-appearance" value={settings.appearance} options={appearances}
          disabled={settings.fontLoading || settings.appearanceSaving}
          className="max-w-full shrink-0 [&>button]:flex-none [&>button]:whitespace-nowrap [&>button]:px-3"
          onChange={value => { void settings.changeAppearance(value); }} />
      </SettingsRow>
    </SettingsGroup>

    <SettingsGroup id="settings-startup-group" title="启动">
      <SettingsRow label={<label htmlFor="settings-autostart">登录时启动 Bento</label>}
        description={<>
          <p id="settings-autostart-help">登录 macOS 后在菜单栏中静默运行，不打开窗口</p>
          {autostartReason && <p id="settings-autostart-reason">{autostartReason}</p>}
        </>}
        error={settings.autostartError && <p role="alert" className="h1-list-note" data-autostart-error>{settings.autostartError}</p>}>
        <Switch id="settings-autostart" checked={settings.autostart?.enabled ?? false}
          disabled={!settings.autostart?.available || settings.autostartSaving}
          aria-describedby={autostartReason ? 'settings-autostart-help settings-autostart-reason' : 'settings-autostart-help'}
          onCheckedChange={enabled => { void settings.changeAutostart(enabled); }} />
      </SettingsRow>
      <FeatureRow settings={settings} feature="tray.enabled" id="settings-show-tray" label="在菜单栏显示" />
    </SettingsGroup>
  </>;
}

function TerminalSettings({ settings }: { settings: SettingsController }) {
  return <>
    <SettingsGroup id="settings-display-group" title="显示">
      <SettingsRow label={<label htmlFor="settings-font-size">字号</label>}
        description={<p id="settings-font-help">立即应用到所有终端，默认 {DEFAULT_TERMINAL_FONT_SIZE} px</p>}
        error={settings.fontError && <p role="alert" className="h1-list-note" data-font-size-error>{settings.fontError}</p>}>
        <Select id="settings-font-size" label="字号" value={String(settings.fontSize)} options={fontSizes} clearRowsWhenAbove
          disabled={settings.fontLoading || settings.fontSaving} aria-describedby="settings-font-help" data-settings-font-size
          onValueChange={value => { void settings.changeFontSize(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
      </SettingsRow>
      <SettingsRow label={<label htmlFor="settings-max-terminals">每个项目最多终端数</label>}
        description={<p id="settings-max-terminals-help">调低不会关闭已打开的终端</p>}
        error={settings.maxError && <p role="alert" className="h1-list-note" data-max-terminals-error>{settings.maxError}</p>}>
        <Select id="settings-max-terminals" label="每个项目最多终端数" value={String(settings.maxPerProject)} options={terminalLimits} clearRowsWhenAbove
          disabled={settings.fontLoading || settings.maxSaving} aria-describedby="settings-max-terminals-help" data-settings-max-terminals
          onValueChange={value => { void settings.changeMaxPerProject(Number(value)); }} className="w-24 shrink-0 tabular-nums" />
      </SettingsRow>
    </SettingsGroup>

    <SettingsGroup id="settings-terminal-features-group" title="功能">
      <FeatureRow settings={settings} feature="terminal.quickCommands" label="常用命令"
        description="终端底部的命令栏。关闭后不再显示，已保存的命令会保留" />
    </SettingsGroup>
  </>;
}

export function SettingsDialog({ settings, updater, category, onCategoryChange, onClose, onCheckUpdates }: {
  settings: SettingsController;
  updater: UpdaterController;
  category: SettingsCategory;
  onCategoryChange: (category: SettingsCategory) => void;
  onClose: () => void;
  onCheckUpdates: () => void;
}) {
  const activeIndex = categories.findIndex(item => item.id === category);
  const version = updater.currentVersion || '…';
  const panels: Record<SettingsCategory, ReactNode> = {
    general: <GeneralSettings settings={settings} />,
    terminal: <TerminalSettings settings={settings} />,
    notifications: <SettingsGroup id="settings-notification-group" title="通知">
      <FeatureRow settings={settings} feature="terminal.commandNotices" label="命令完成通知" description="较长的命令跑完时提醒你" />
      <FeatureRow settings={settings} feature="terminal.agentNotices" label="AI 会话提醒" description="Claude / Codex 一轮结束时提醒你" />
    </SettingsGroup>,
    about: <div className="settings-group">
      <SettingsRow label={<>版本 <span className="tabular-nums">v{version}</span></>}>
        <Button variant="outline" size="sm" className="shrink-0" onClick={onCheckUpdates}>检查更新</Button>
      </SettingsRow>
    </div>,
  };

  const onCategoryKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const count = categories.length;
    let next: number;
    switch (event.key) {
      case 'ArrowUp': next = (activeIndex - 1 + count) % count; break;
      case 'ArrowDown': next = (activeIndex + 1) % count; break;
      case 'Home': next = 0; break;
      case 'End': next = count - 1; break;
      default: return;
    }
    event.preventDefault();
    onCategoryChange(categories[next].id);
    event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  };

  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className="h-[560px] max-h-[90dvh] w-[90vw] max-w-[780px] grid-cols-[clamp(112px,24.4%,190px)_minmax(0,1fr)] gap-0 rounded-2xl p-0"
      closeClassName="right-5 top-5" data-settings-dialog>
      <aside className="settings-sidebar flex min-h-0 flex-col overflow-y-auto rounded-l-[15px] px-3 py-5">
        <p className="mb-5 px-2.5 text-[15px] font-semibold">设置</p>
        <div role="tablist" aria-label="设置分类" aria-orientation="vertical" className="grid gap-1" onKeyDown={onCategoryKeyDown}>
          {categories.map(({ id, label, icon: Icon }) => <button key={id} type="button" role="tab"
            id={`settings-tab-${id}`} aria-controls={`settings-panel-${id}`} aria-selected={category === id}
            tabIndex={category === id ? 0 : -1} onClick={event => {
              event.currentTarget.focus();
              onCategoryChange(id);
            }}
            className="settings-tab flex h-9 w-full items-center gap-2.5 rounded-[8px] px-2.5 text-left text-[13px] text-muted-foreground hover:text-foreground aria-selected:font-semibold aria-selected:text-foreground">
            <Icon size={15} className="shrink-0" aria-hidden="true" />
            {label}
          </button>)}
        </div>
        <p className="mt-auto px-2.5 pt-5 text-[11px] tabular-nums text-muted-foreground">Bento v{version}</p>
      </aside>

      <div className="flex min-h-0 min-w-0 flex-col">
        <DialogHeader className="shrink-0 pb-5 pl-[clamp(12px,3vw,30px)] pr-14 pt-6">
          <DialogTitle className="text-[23px] leading-8"><span className="sr-only">设置 · </span>{categories[activeIndex].label}</DialogTitle>
          <DialogDescription className="sr-only">管理外观、启动、终端、通知和更新设置。</DialogDescription>
        </DialogHeader>
        {categories.map(({ id }) => <div key={id} id={`settings-panel-${id}`} role="tabpanel"
          aria-labelledby={`settings-tab-${id}`} hidden={category !== id} tabIndex={category === id ? 0 : undefined}
          className="min-h-0 flex-1 space-y-[18px] overflow-y-auto overscroll-contain px-[clamp(12px,3vw,30px)] pb-6">
          {category === id && panels[id]}
        </div>)}
      </div>
    </DialogContent>
  </Dialog>;
}
