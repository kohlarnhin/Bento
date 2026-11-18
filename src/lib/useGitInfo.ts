import { useEffect, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { GitInfo, ProjectConfig } from '../types';

const REFRESH_MS = 15_000;

interface Entry { id: string; path: string; refs: number; pending: boolean; updated: number }
const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();
let snapshot: Record<string, GitInfo | null> = {};
let timer: ReturnType<typeof setInterval> | undefined;
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const getSnapshot = () => snapshot;
const publish = (id: string, info: GitInfo | null) => {
  if (JSON.stringify(snapshot[id]) === JSON.stringify(info)) return;
  snapshot = { ...snapshot, [id]: info };
  listeners.forEach(listener => listener());
};

function refresh(force = false) {
  if (document.visibilityState === 'hidden') return;
  for (const entry of entries.values()) {
    if (!entry.refs || entry.pending || (!force && Date.now() - entry.updated < REFRESH_MS)) continue;
    entry.pending = true;
    void invoke<GitInfo | null>('git_info', { id: entry.id }).catch(() => null).then(info => {
      if (entries.get(entry.id) === entry) publish(entry.id, info);
    }).finally(() => { entry.pending = false; entry.updated = Date.now(); });
  }
}
const onVisibilityChange = () => refresh(true);

function retain(id: string, path: string) {
  let entry = entries.get(id);
  if (!entry || entry.path !== path) {
    entry = { id, path, refs: 0, pending: false, updated: 0 };
    entries.set(id, entry);
    publish(id, null);
  }
  entry.refs++;
  if (!timer) {
    timer = setInterval(() => refresh(true), REFRESH_MS);
    document.addEventListener('visibilitychange', onVisibilityChange);
  }
  refresh();
  const retained = entry;
  return () => {
    retained.refs--;
    // Keep an in-flight initial read shared across StrictMode's effect remount.
    setTimeout(() => {
      if (!retained.refs && entries.get(id) === retained) entries.delete(id);
      if (![...entries.values()].some(item => item.refs)) {
        clearInterval(timer);
        timer = undefined;
        document.removeEventListener('visibilitychange', onVisibilityChange);
      }
    }, 0);
  };
}

/** Overview and sidebar share the same requests, visibility pause and 15s cache. */
export function useGitInfo(projectId: string, path: string) {
  const infos = useSyncExternalStore(subscribe, getSnapshot);
  useEffect(() => retain(projectId, path), [projectId, path]);
  return infos[projectId] ?? null;
}

export function useProjectGitInfos(projects: ProjectConfig[]) {
  const infos = useSyncExternalStore(subscribe, getSnapshot);
  const paths = JSON.stringify(projects.map(project => [project.id, project.path]));
  useEffect(() => {
    const release = (JSON.parse(paths) as [string, string][]).map(([id, path]) => retain(id, path));
    return () => release.forEach(stop => stop());
  }, [paths]);
  return infos;
}
