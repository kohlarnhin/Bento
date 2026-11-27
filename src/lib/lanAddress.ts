import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

// The Mac's LAN IPv4 address, shared by every service row and refreshed when the window regains focus
// (Wi-Fi changes) or once a minute.
let cached: string | null = null;
let pending: Promise<string | null> | undefined;
const listeners = new Set<(ip: string | null) => void>();

function load() {
  pending ??= invoke<string | null>('local_network_ip').catch(() => null).then(ip => {
    pending = undefined;
    if (ip !== cached) { cached = ip; listeners.forEach(listener => listener(ip)); }
    return ip;
  });
  return pending;
}

export function useLanAddress() {
  const [ip, setIp] = useState(cached);
  useEffect(() => {
    listeners.add(setIp);
    const refresh = () => { void load(); };
    refresh();
    window.addEventListener('focus', refresh);
    const timer = window.setInterval(refresh, 60_000);
    return () => { listeners.delete(setIp); window.removeEventListener('focus', refresh); window.clearInterval(timer); };
  }, []);
  return ip;
}

export function copyText(text: string, success = '已复制') {
  void navigator.clipboard.writeText(text).then(() => toast.success(success), error => toast.error(String(error)));
}
