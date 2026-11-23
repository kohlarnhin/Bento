import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DEFAULT_TERMINAL_FONT_SIZE } from '../lib/terminalAppearance';
import { setTerminalFontSize } from '../lib/terminalRegistry';

interface Settings {
  'terminal.fontSize': number;
  'terminal.maxPerProject': number;
  'autostart.defaultApplied': boolean;
  'appearance.theme': Appearance;
  'terminal.restore': boolean;
  'terminal.quickCommands'?: boolean;
  'terminal.commandNotices': boolean;
  'terminal.agentNotices': boolean;
  'tray.enabled': boolean;
}

const defaults = {
  'terminal.restore': true,
  'terminal.quickCommands': true,
  'terminal.commandNotices': true,
  'terminal.agentNotices': true,
  'tray.enabled': true,
};
type Feature = keyof typeof defaults;
type Appearance = 'system' | 'light' | 'dark';

interface AutostartStatus {
  available: boolean;
  enabled: boolean;
  reason?: string;
}

// React StrictMode remounts share the initial reads, so terminals are fitted only once.
let initialSettings: Promise<Settings> | undefined;
let initialAutostart: Promise<AutostartStatus> | undefined;

export function useSettings() {
  const [appearance, setAppearance] = useState<Appearance>('system');
  const [appearanceSaving, setAppearanceSaving] = useState(false);
  const [appearanceError, setAppearanceError] = useState<string | null>(null);
  const appearancePending = useRef(false);
  const [systemDark, setSystemDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const colorScheme = appearance === 'system' ? (systemDark ? 'dark' : 'light') : appearance;
  useEffect(() => {
    const scheme = window.matchMedia('(prefers-color-scheme: dark)');
    const update = () => setSystemDark(scheme.matches);
    scheme.addEventListener('change', update);
    update();
    return () => scheme.removeEventListener('change', update);
  }, []);
  useLayoutEffect(() => {
    document.documentElement.classList.toggle('dark', colorScheme === 'dark');
  }, [colorScheme]);
  const [fontSize, setFontSize] = useState(DEFAULT_TERMINAL_FONT_SIZE);
  const [fontLoading, setFontLoading] = useState(true);
  const [fontSaving, setFontSaving] = useState(false);
  const [fontError, setFontError] = useState<string | null>(null);
  const [maxPerProject, setMaxPerProject] = useState(2);
  const [maxSaving, setMaxSaving] = useState(false);
  const [maxError, setMaxError] = useState<string | null>(null);
  const [features, setFeatures] = useState(defaults);
  const [featureSaving, setFeatureSaving] = useState<Feature>();
  const [featureError, setFeatureError] = useState<{ key: Feature; message: string } | null>(null);
  const featurePending = useRef(false);
  const [autostart, setAutostart] = useState<AutostartStatus | null>(null);
  const [autostartSaving, setAutostartSaving] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const requestedFontSize = useRef(DEFAULT_TERMINAL_FONT_SIZE);
  const savedFontSize = useRef(DEFAULT_TERMINAL_FONT_SIZE);
  const fontRevision = useRef(0);
  const fontSaveQueue = useRef(Promise.resolve());
  const maxPending = useRef(false);
  const autostartPending = useRef(false);

  useEffect(() => {
    let active = true;
    initialSettings ??= invoke<Settings>('get_settings');
    initialAutostart ??= invoke<AutostartStatus>('autostart_status');
    void initialSettings.then(settings => {
      if (!active) return;
      savedFontSize.current = settings['terminal.fontSize'];
      if (fontRevision.current === 0) {
        requestedFontSize.current = settings['terminal.fontSize'];
        setFontSize(settings['terminal.fontSize']);
        setTerminalFontSize(settings['terminal.fontSize']);
      }
      setMaxPerProject(settings['terminal.maxPerProject']);
      setAppearance(settings['appearance.theme'] ?? 'system');
      setFeatures({
        'terminal.restore': settings['terminal.restore'] ?? true,
        'terminal.quickCommands': settings['terminal.quickCommands'] ?? defaults['terminal.quickCommands'],
        'terminal.commandNotices': settings['terminal.commandNotices'] ?? true,
        'terminal.agentNotices': settings['terminal.agentNotices'] ?? true,
        'tray.enabled': settings['tray.enabled'] ?? true,
      });
    }).catch(error => { if (active) setFontError(`读取设置失败：${error}`); })
      .finally(() => { if (active) setFontLoading(false); });
    void initialAutostart.then(status => { if (active) setAutostart(status); })
      .catch(error => { if (active) setAutostartError(`读取启动设置失败：${error}`); });
    return () => { active = false; };
  }, []);

  const changeAppearance = async (next: Appearance) => {
    if (appearancePending.current || next === appearance) return;
    appearancePending.current = true;
    setAppearanceSaving(true);
    setAppearanceError(null);
    setAppearance(next);
    try {
      await invoke('set_setting', { key: 'appearance.theme', value: next });
    } catch (error) {
      setAppearance(appearance);
      setAppearanceError(`保存外观失败：${error}`);
    } finally {
      appearancePending.current = false;
      setAppearanceSaving(false);
    }
  };

  const changeFontSize = useCallback((next: number | ((current: number) => number)) => {
    const size = typeof next === 'function' ? next(requestedFontSize.current) : next;
    if (!Number.isInteger(size) || size < 12 || size > 20 || size === requestedFontSize.current) return Promise.resolve();
    const revision = ++fontRevision.current;
    requestedFontSize.current = size;
    setFontSaving(true);
    setFontError(null);
    setFontSize(size);
    setTerminalFontSize(size);
    // Keep the display immediate and serialize writes, including rapid zooms.
    fontSaveQueue.current = fontSaveQueue.current.then(async () => {
      try {
        await initialSettings?.catch(() => undefined);
        await invoke('set_setting', { key: 'terminal.fontSize', value: size });
        savedFontSize.current = size;
      } catch (error) {
        if (revision === fontRevision.current) {
          requestedFontSize.current = savedFontSize.current;
          setFontSize(savedFontSize.current);
          setTerminalFontSize(savedFontSize.current);
          setFontError(`保存字号失败：${error}`);
        }
      } finally {
        if (revision === fontRevision.current) setFontSaving(false);
      }
    });
    return fontSaveQueue.current;
  }, []);

  const changeMaxPerProject = useCallback(async (max: number) => {
    if (maxPending.current || !Number.isInteger(max) || max < 1 || max > 6) return;
    maxPending.current = true;
    const previous = maxPerProject;
    setMaxSaving(true);
    setMaxError(null);
    setMaxPerProject(max);
    try {
      await invoke('set_setting', { key: 'terminal.maxPerProject', value: max });
    } catch (error) {
      setMaxPerProject(previous);
      setMaxError(`保存终端数量失败：${error}`);
    } finally {
      maxPending.current = false;
      setMaxSaving(false);
    }
  }, [maxPerProject]);

  const changeAutostart = useCallback(async (enabled: boolean) => {
    if (!autostart?.available || autostartPending.current) return;
    autostartPending.current = true;
    const previous = autostart;
    setAutostartSaving(true);
    setAutostartError(null);
    setAutostart({ ...previous, enabled });
    try {
      setAutostart(await invoke<AutostartStatus>('set_autostart', { enabled }));
    } catch (error) {
      setAutostart(previous);
      setAutostartError(`更改启动设置失败：${error}`);
    } finally {
      autostartPending.current = false;
      setAutostartSaving(false);
    }
  }, [autostart]);

  const changeFeature = async (key: Feature, value: boolean) => {
    if (featurePending.current) return;
    featurePending.current = true;
    setFeatureSaving(key);
    setFeatureError(null);
    try {
      await invoke('set_setting', { key, value });
      setFeatures(previous => ({ ...previous, [key]: value }));
    } catch (error) {
      setFeatureError({ key, message: String(error) });
    } finally {
      featurePending.current = false;
      setFeatureSaving(undefined);
    }
  };

  return { fontSize, fontLoading, fontSaving, fontError, changeFontSize,
    maxPerProject, maxSaving, maxError, changeMaxPerProject,
    features, featureSaving, featureError, changeFeature,
    appearance, colorScheme, appearanceSaving, appearanceError, changeAppearance,
    autostart, autostartSaving, autostartError, changeAutostart };
}

export type SettingsController = ReturnType<typeof useSettings>;
