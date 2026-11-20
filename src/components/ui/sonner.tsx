import { Toaster as Sonner } from 'sonner';

export function Toaster({ theme }: { theme: 'light' | 'dark' }) {
  return <Sonner theme={theme} position="bottom-right" closeButton toastOptions={{
    classNames: { closeButton: 'button-neutral' },
    style: {
      background: 'var(--h1-surface)', color: 'hsl(var(--foreground))', border: '0',
      borderRadius: '16px', boxShadow: 'var(--selected-shadow)', fontFamily: 'var(--font-ui)', fontSize: '13px',
    },
  }} />;
}
