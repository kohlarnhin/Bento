import { Toaster as Sonner } from 'sonner';

export function Toaster({ theme }: { theme: 'light' | 'dark' }) {
  return <Sonner theme={theme} position="bottom-right" closeButton toastOptions={{ style: {
    background: 'hsl(var(--popover))', color: 'hsl(var(--popover-foreground))', borderColor: 'hsl(var(--border))',
    fontFamily: 'inherit', fontSize: '13px',
  } }} />;
}
