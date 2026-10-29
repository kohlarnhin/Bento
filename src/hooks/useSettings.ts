import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DEFAULT_TERMINAL_FONT_SIZE } from '../lib/terminalAppearance';
import { setTerminalFontSize } from '../lib/terminalRegistry';

interface Settings {
  'terminal.fontSize': number;
  'terminal.maxPerProject': number;
  'autostart.defaultApplied': boolean;
}

interface AutostartStatus {
  available: boolean;
  enabled: boolean;
  reason?: string;
}

// React StrictMode remounts share the initial reads, so terminals are fitted only once.
let initialSettings: Promise<Settings> | undefined;
let initialAutostart: Promise<AutostartStatus> | undefined;

export function useSettings() {
  const [fontSize, setFontSize] = useState(DEFAULT_TERMINAL_FONT_SIZE);
  const [fontLoading, setFontLoading] = useState(true);
  const [fontSaving, setFontSaving] = useState(false);
  const [fontError, setFontError] = useState<string | null>(null);
  const [maxPerProject, setMaxPerProject] = useState(2);
  const [maxSaving, setMaxSaving] = useState(false);
  const [maxError, setMaxError] = useState<string | null>(null);
  const [autostart, setAutostart] = useState<AutostartStatus | null>(null);
  const [autostartSaving, setAutostartSaving] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const fontPending = useRef(false);
  const maxPending = useRef(false);
  const autostartPending = useRef(false);

  useEffect(() => {
    let active = true;
    initialSettings ??= invoke<Settings>('get_settings');
    initialAutostart ??= invoke<AutostartStatus>('autostart_status');
    void initialSettings.then(settings => {
      if (!active) return;
      setFontSize(settings['terminal.fontSize']);
      setMaxPerProject(settings['terminal.maxPerProject']);
      setTerminalFontSize(settings['terminal.fontSize']);
    }).catch(error => { if (active) setFontError(`读取设置失败：${error}`); })
      .finally(() => { if (active) setFontLoading(false); });
    void initialAutostart.then(status => { if (active) setAutostart(status); })
      .catch(error => { if (active) setAutostartError(`读取启动设置失败：${error}`); });
    return () => { active = false; };
  }, []);

  const changeFontSize = useCallback(async (size: number) => {
    if (fontPending.current || !Number.isInteger(size) || size < 12 || size > 20) return;
    fontPending.current = true;
    const previous = fontSize;
    setFontSaving(true);
    setFontError(null);
    setFontSize(size);
    setTerminalFontSize(size);
    try {
      await invoke('set_setting', { key: 'terminal.fontSize', value: size });
    } catch (error) {
      setFontSize(previous);
      setTerminalFontSize(previous);
      setFontError(`保存字号失败：${error}`);
    } finally {
      fontPending.current = false;
      setFontSaving(false);
    }
  }, [fontSize]);

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

  return { fontSize, fontLoading, fontSaving, fontError, changeFontSize,
    maxPerProject, maxSaving, maxError, changeMaxPerProject,
    autostart, autostartSaving, autostartError, changeAutostart };
}

export type SettingsController = ReturnType<typeof useSettings>;
