import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export function LogText({ text }: { text: string }) {
  return text.split(/(https?:\/\/[^\s<>"'\x1b]+)/gu).map((part, index) => {
    if (index % 2 === 0) return part;
    return <a key={index} className="log-url" href={part} onClick={event => {
      event.preventDefault();
      void invoke('open_url', { url: part }).catch(error => toast.error(`打开链接失败：${error}`));
    }}>{part}</a>;
  });
}
