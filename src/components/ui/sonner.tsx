import { Toaster as Sonner } from 'sonner';

export function Toaster({ theme }: { theme: 'light' | 'dark' }) {
  return <Sonner theme={theme} position="bottom-right" closeButton toastOptions={{ style: {
    background: 'var(--h1-surface)', color: 'hsl(var(--foreground))', border: '.5px solid var(--h1-line)',
    borderRadius: '14px', boxShadow: 'var(--h1-popup-shadow)', backdropFilter: 'blur(22px)',
    fontFamily: 'var(--font-ui)', fontSize: '13px',
  } }} />;
}
