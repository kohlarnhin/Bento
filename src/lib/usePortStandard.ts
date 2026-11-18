import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { PortStandardCheck, Service } from '../types';

export function usePortStandard(path: string, startCommand: string, services: Service[], enabled = true) {
  const request = enabled && path.trim() && services.length
    ? JSON.stringify({ path, startCommand, services }) : '';
  const [result, setResult] = useState<{ request: string; data?: PortStandardCheck; error?: string }>();
  useEffect(() => {
    if (!request) return;
    let active = true;
    let sequence = 0;
    const check = () => {
      const current = ++sequence;
      void invoke<PortStandardCheck>('check_project_port_standard', JSON.parse(request))
        .then(data => { if (active && current === sequence) setResult({ request, data }); })
        .catch(error => { if (active && current === sequence) setResult({ request, error: String(error) }); });
    };
    check();
    window.addEventListener('focus', check);
    return () => { active = false; window.removeEventListener('focus', check); };
  }, [request]);

  return result?.request === request ? result : undefined;
}
