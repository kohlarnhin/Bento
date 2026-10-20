import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { GitInfo } from '../types';

const REFRESH_MS = 15_000;

/** Git state of a project's directory; null when it isn't a repository (or git is unavailable). */
export function useGitInfo(projectId: string, path: string) {
  const [info, setInfo] = useState<GitInfo | null>(null);
  useEffect(() => {
    let active = true;
    let pending = false;
    setInfo(null);
    const refresh = () => {
      if (pending || document.visibilityState === 'hidden') return;
      pending = true;
      invoke<GitInfo | null>('git_info', { id: projectId })
        .then(next => { if (active) setInfo(next); })
        .catch(() => { if (active) setInfo(null); })
        .finally(() => { pending = false; });
    };
    refresh();
    const timer = setInterval(refresh, REFRESH_MS);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      active = false;
      clearInterval(timer);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [projectId, path]);
  return info;
}
