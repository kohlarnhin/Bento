import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { projectInitial } from '../lib/sidebarModel';
import { StatusDot } from './StatusDot';

type Row = { id: string; name: string; runnable: boolean; port: number | null; status: 'starting' | 'running' | 'stopped' | 'error'; startedAt: number | null; agentDetail?: string | null };
type Pane = { running: number; canStop: boolean; rows: Row[] };

function duration(startedAt: number, now: number) {
  const minutes = Math.max(0, Math.floor((now - startedAt) / 60000));
  if (minutes < 1) return '不到 1 分钟';
  return minutes < 60 ? minutes + ' 分钟' : Math.floor(minutes / 60) + ' 小时 ' + minutes % 60 + ' 分钟';
}

export function TrayPanel() {
  const [view, setView] = useState<Pane>();
  const [time, setTime] = useState(Date.now);
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState('');
  const load = useCallback(async () => {
    const value = await invoke<Pane>('get_tray_panel');
    setView(value);
    setTime(Date.now());
  }, []);
  useEffect(() => {
    const theme = window.matchMedia('(prefers-color-scheme: dark)');
    const updateTheme = () => document.documentElement.classList.toggle('dark', theme.matches);
    updateTheme();
    theme.addEventListener('change', updateTheme);
    const refresh = () => { void load().catch(error => setError(String(error))); };
    const unsubscribe = listen('tray-panel-refresh', refresh);
    void unsubscribe.then(refresh);
    const timer = setInterval(() => { if (document.visibilityState === 'visible') setTime(Date.now()); }, 15000);
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void invoke('tray_panel_action', { action: 'hide' });
    };
    window.addEventListener('keydown', escape);
    return () => {
      clearInterval(timer);
      window.removeEventListener('keydown', escape);
      theme.removeEventListener('change', updateTheme);
      void unsubscribe.then(unlisten => unlisten());
    };
  }, [load]);

  const action = async (action: string, projectId?: string) => {
    setError('');
    try { await invoke('tray_panel_action', { action, projectId }); }
    catch (error) { setError(String(error)); }
  };
  const toggle = async (row: Row) => {
    setBusy(row.id);
    setError('');
    try {
      await invoke(row.status === 'stopped' ? 'start_project' : 'stop_project', { id: row.id });
      await invoke('note_tray_project', { projectId: row.id });
      await load();
    } catch (error) { setError(String(error)); }
    finally { setBusy(undefined); }
  };

  return <main className="tray-panel" data-tray-panel>
    <header><strong>Bento</strong><span>{view?.running ?? 0} 个运行中</span></header>
    <div className="tray-projects" aria-label="项目">
      {view?.rows.map(row => {
        const detail = row.status === 'stopped' ? (row.runnable ? '已停止' : '无需运行') : row.status === 'starting' ? '启动中' : row.status === 'error' ? '运行异常'
          : [row.port == null ? undefined : ':' + row.port, row.startedAt == null ? undefined : duration(row.startedAt, time)].filter(Boolean).join(' · ');
        return <div className="tray-project" key={row.id} data-tray-project={row.id} data-tray-status={row.status}>
          <button type="button" className="tray-project-name" onClick={() => { void action('show', row.id); }}>
            <span className="tray-project-avatar" aria-hidden="true">{projectInitial(row.name)}</span>
            <span className="min-w-0"><b title={row.name}>{row.name}</b><small>{row.agentDetail ?? (detail || '运行中')}</small></span>
          </button>
          <StatusDot tone={row.status} aria-label={row.status === 'running' ? '运行中' : detail} />
          {(row.runnable || row.status !== 'stopped') && <button type="button" className="h1-button" data-tray-toggle={row.id} disabled={busy != null}
            onClick={() => { void toggle(row); }}>{row.status === 'stopped' ? '启动' : '停止'}</button>}
        </div>;
      })}
      {view?.rows.length === 0 && <p className="tray-empty">没有运行中的项目</p>}
    </div>
    <footer>
      <div className="tray-footer-actions">
        <button type="button" className="h1-button" onClick={() => { void action('show'); }}>打开 Bento</button>
        <button type="button" className="h1-button h1-button-primary" disabled={!view?.canStop}
          onClick={() => { void action('stop-all'); }}>全部停止</button>
      </div>
      {error ? <p className="tray-error" role="alert" title={error}>{error}</p> : <p>关掉主窗口，服务和终端照常运行</p>}
    </footer>
  </main>;
}
