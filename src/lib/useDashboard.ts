import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { DashboardGit, DashboardHealth, DashboardSnapshot, ProjectConfig, ProjectRuntimeStatus, ProjectView } from '../types';

function useDashboardPoll(task: () => void, delay: number, events: { focus?: boolean } = {}) {
  const action = useRef(task);
  action.current = task;
  const onFocus = events.focus;
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    const poll = () => { if (document.visibilityState === 'visible') action.current(); };
    const visibility = () => {
      clearInterval(timer);
      if (document.visibilityState !== 'visible' || !delay) return;
      poll();
      timer = setInterval(poll, delay);
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    if (onFocus) window.addEventListener('focus', poll);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', visibility);
      if (onFocus) window.removeEventListener('focus', poll);
    };
  }, [delay, onFocus]);
}

export function useDashboard(project: ProjectConfig, runtime: ProjectRuntimeStatus | undefined, view: ProjectView) {
  const { id, path } = project;
  const [snapshot, setSnapshot] = useState<DashboardSnapshot>();
  const [snapshotError, setSnapshotError] = useState<string>();
  const [git, setGit] = useState<DashboardGit | null>(null);
  const [health, setHealth] = useState<Record<string, DashboardHealth>>({});
  const [now, setNow] = useState(Date.now);
  const lifetime = useRef(0);
  const snapshotRequest = useRef(0);
  const gitPending = useRef<number>();
  const healthPending = useRef<{ instance: number; generation: number }>();
  const preferred = useRef<Record<string, string>>({});
  const healthGeneration = useRef(0);
  const ports = JSON.stringify([project.port, project.services.map(service => [service.id, service.port, service.kind])]);
  const previousLaunch = useRef(runtime?.startedAt);
  const previousInputs = useRef<string>();
  const runtimeKey = JSON.stringify([runtime?.status, runtime?.revision, runtime?.external,
    runtime?.services.map(service => [service.serviceId, service.port, service.listening, service.owner?.pid])]);
  useEffect(() => {
    lifetime.current++;
    return () => { lifetime.current++; };
  }, [id, path]);

  const refresh = useCallback(() => {
    const instance = lifetime.current;
    const request = ++snapshotRequest.current;
    void invoke<DashboardSnapshot>('dashboard_snapshot', { id })
      .then(current => {
        if (instance !== lifetime.current || request !== snapshotRequest.current) return;
        setSnapshot(current);
        setSnapshotError(undefined);
      }).catch(error => {
        if (instance === lifetime.current && request === snapshotRequest.current) setSnapshotError(String(error));
      });
  }, [id]);

  useEffect(() => {
    const inputs = JSON.stringify([path, ports, runtimeKey]);
    if (previousInputs.current != null && previousInputs.current !== inputs && document.visibilityState === 'visible') refresh();
    previousInputs.current = inputs;
    if (previousLaunch.current !== runtime?.startedAt && runtime?.startedAt != null) {
      previousLaunch.current = runtime.startedAt;
      healthGeneration.current++;
      setHealth({});
    }
  }, [refresh, runtimeKey, runtime?.startedAt, path, ports]);

  useDashboardPoll(refresh, 5000);
  useDashboardPoll(() => setNow(Date.now()), 15_000);
  useDashboardPoll(() => {
    const instance = lifetime.current;
    if (gitPending.current === instance) return;
    gitPending.current = instance;
    void invoke<DashboardGit | null>('dashboard_git', { id })
      .then(value => { if (instance === lifetime.current) setGit(value); })
      .catch(() => {})
      .finally(() => { if (gitPending.current === instance) gitPending.current = undefined; });
  }, view === 'terminal' ? 0 : 15_000, { focus: view !== 'terminal' });

  const listening = snapshot?.services.some(service => service.state === 'listening') ?? false;
  useDashboardPoll(() => {
    if (!listening) return;
    const instance = lifetime.current;
    const generation = healthGeneration.current;
    if (healthPending.current?.instance === instance && healthPending.current.generation === generation) return;
    const request = { instance, generation };
    healthPending.current = request;
    void invoke<Record<string, DashboardHealth>>('dashboard_health', { id, preferred: preferred.current })
      .then(results => {
        if (instance !== lifetime.current || generation !== healthGeneration.current) return;
        for (const [key, record] of Object.entries(results)) {
          if (record.code != null && record.code >= 200 && record.code < 300) preferred.current[key] = record.path;
        }
        setHealth(previous => ({ ...previous, ...results }));
      }).catch(() => {})
      .finally(() => { if (healthPending.current === request) healthPending.current = undefined; });
  }, listening && view === 'overview' ? 10_000 : 0);

  useEffect(() => {
    const unlisten = listen<string>('dashboard-run-saved', ({ payload }) => {
      if (payload === id && document.visibilityState === 'visible') refresh();
    });
    return () => { void unlisten.then(stop => stop()); };
  }, [id, refresh]);

  return { snapshot, snapshotError, git, health, now, refresh };
}
