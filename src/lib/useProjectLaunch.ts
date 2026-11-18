import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ProjectLaunchConfig, ProjectLaunchPlan } from '../types';

export function useProjectLaunch(config: ProjectLaunchConfig, enabled = true) {
  const request = enabled && config.path.trim() ? JSON.stringify({
    path: config.path, startCommand: config.startCommand, port: config.port,
    services: config.services, injectPorts: config.injectPorts, env: config.env,
  }) : '';
  const [result, setResult] = useState<{ request: string; data?: ProjectLaunchPlan; error?: string }>();
  useEffect(() => {
    if (!request) return;
    let active = true;
    let sequence = 0;
    const preview = () => {
      const current = ++sequence;
      void invoke<ProjectLaunchPlan>('preview_project_launch', { config: JSON.parse(request) })
        .then(data => { if (active && sequence === current) setResult({ request, data }); })
        .catch(error => { if (active && sequence === current) setResult({ request, error: String(error) }); });
    };
    const timer = setTimeout(preview, 160);
    window.addEventListener('focus', preview);
    return () => {
      active = false;
      clearTimeout(timer);
      window.removeEventListener('focus', preview);
    };
  }, [request]);
  return result?.request === request ? result : undefined;
}
