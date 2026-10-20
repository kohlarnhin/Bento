import { useCallback, useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater';
import { toast } from 'sonner';

export type UpdaterStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'downloading' | 'installing' | 'restarting' | 'error';

const CHECK_TIMEOUT_MS = 15_000;
const DOWNLOAD_TIMEOUT_MS = 5 * 60_000;
const AUTO_CHECK_INTERVAL_MS = 6 * 60 * 60_000;

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error || '发生未知错误');
}

export function useUpdater() {
  const [currentVersion, setCurrentVersion] = useState('');
  const [status, setStatus] = useState<UpdaterStatus>('idle');
  const [update, setUpdate] = useState<Update | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState<number | null>(null);
  const [isModalOpen, setIsModalOpen] = useState(false);
  const updateRef = useRef<Update | null>(null);
  const checking = useRef(false);
  const installing = useRef(false);
  const installed = useRef(false);
  const modalOpen = useRef(false);
  const manualCheck = useRef(false);
  const notified = useRef(new Set<string>());

  const openModal = useCallback(() => {
    modalOpen.current = true;
    setIsModalOpen(true);
  }, []);

  const notifyAvailable = useCallback(async () => {
    const available = updateRef.current;
    if (!available || notified.current.has(available.version) || installing.current) return;
    // A hidden tray window should receive its toast when it is shown again.
    try {
      if (!await getCurrentWindow().isVisible()) return;
    } catch { return; }
    if (installing.current || notified.current.has(available.version) || updateRef.current !== available) return;
    notified.current.add(available.version);
    toast(`Bento v${available.version} 可用`, {
      id: `bento-update-${available.version}`,
      duration: 8_000,
      action: { label: '查看', onClick: openModal },
    });
  }, [openModal]);

  const checkForUpdates = useCallback(async (manual: boolean, refresh = false) => {
    if (manual) openModal();
    if (installing.current || (!manual && modalOpen.current)) return;
    if (manual) manualCheck.current = true;
    if (checking.current) {
      if (manual) { setStatus('checking'); setError(null); }
      return;
    }
    if (manual && updateRef.current && !refresh) return;
    if (!import.meta.env.PROD) {
      setStatus('error');
      setError('开发模式不支持更新');
      return;
    }

    checking.current = true;
    manualCheck.current = manual;
    if (manual) {
      setStatus('checking');
      setError(null);
    }
    try {
      const next = await check({ timeout: CHECK_TIMEOUT_MS });
      const previous = updateRef.current;
      updateRef.current = next;
      setUpdate(next);
      setError(null);
      setStatus(next ? 'available' : 'up-to-date');
      if (previous && previous !== next) void previous.close().catch(() => undefined);
      if (next && !manualCheck.current) void notifyAvailable();
    } catch (checkError) {
      if (manualCheck.current) {
        setStatus('error');
        setError(`检查更新失败：${message(checkError)}`);
      }
      // Background failures preserve the current indicator and never open a dialog/toast.
    } finally {
      checking.current = false;
      manualCheck.current = false;
    }
  }, [notifyAvailable, openModal]);

  const installUpdate = useCallback(async () => {
    const pending = updateRef.current;
    if (!pending || installing.current || checking.current) return;
    installing.current = true;
    setError(null);
    openModal();
    let downloaded = 0;
    let total: number | undefined;
    const progress = (event: DownloadEvent) => {
      if (event.event === 'Started') {
        total = event.data.contentLength;
        setDownloadProgress(total ? 0 : null);
      } else if (event.event === 'Progress') {
        downloaded += event.data.chunkLength;
        if (total) setDownloadProgress(Math.min(99, Math.round(downloaded / total * 100)));
      } else {
        setDownloadProgress(100);
      }
    };
    try {
      if (!installed.current) {
        setStatus('downloading');
        setDownloadProgress(null);
        // download() resolves only after signature verification; no processes stop here.
        await pending.download(progress, { timeout: DOWNLOAD_TIMEOUT_MS });
        setStatus('installing');
        await pending.install();
        installed.current = true;
      }
      setStatus('restarting');
      // process:restart uses request_restart(), so RunEvent::Exit performs normal cleanup.
      await relaunch();
    } catch (installError) {
      installing.current = false;
      setStatus('error');
      const detail = message(installError);
      setError(/permission denied|read.only|not permitted|not writable|failed to move.*app|cross.device|os error (13|18|30)|AppTranslocation/i.test(detail)
        ? '请先把 Bento 拖到「应用程序」文件夹后再更新'
        : `${installed.current ? '重启' : '应用更新'}失败：${detail}`);
    }
  }, [openModal]);

  const checkNow = useCallback(() => {
    if (installed.current) { openModal(); return Promise.resolve(); }
    return checkForUpdates(true, false);
  }, [checkForUpdates, openModal]);

  const retry = useCallback(() => installed.current ? installUpdate() : checkForUpdates(true, true), [checkForUpdates, installUpdate]);
  const closeModal = useCallback(() => {
    if (installing.current) return;
    modalOpen.current = false;
    setIsModalOpen(false);
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    void getVersion().then(version => { if (!disposed) setCurrentVersion(version); }).catch(() => undefined);
    const menu = listen('bento-check-updates', () => { void checkNow(); }).catch(() => () => undefined);
    const focus = getCurrentWindow().onFocusChanged(({ payload }) => {
      if (payload && import.meta.env.PROD) void notifyAvailable();
    }).catch(() => () => undefined);
    return () => {
      disposed = true;
      void menu.then(unlisten => unlisten()).catch(() => undefined);
      void focus.then(unlisten => unlisten()).catch(() => undefined);
    };
  }, [checkNow, notifyAvailable]);

  useEffect(() => {
    if (!import.meta.env.PROD || !isTauri()) return;
    const timer = window.setTimeout(() => { void checkForUpdates(false); }, 5_000);
    const interval = window.setInterval(() => { void checkForUpdates(false); }, AUTO_CHECK_INTERVAL_MS);
    return () => { window.clearTimeout(timer); window.clearInterval(interval); };
  }, [checkForUpdates]);

  useEffect(() => () => {
    if (!installing.current) void updateRef.current?.close().catch(() => undefined);
  }, []);

  return {
    currentVersion, status, update, error, downloadProgress, isModalOpen,
    isBusy: status === 'downloading' || status === 'installing' || status === 'restarting',
    checkNow, installUpdate, retry, closeModal,
  };
}

export type UpdaterController = ReturnType<typeof useUpdater>;
