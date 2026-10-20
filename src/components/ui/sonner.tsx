import { Toaster as Sonner } from 'sonner';

export function Toaster() {
  return <Sonner theme="light" position="bottom-right" closeButton toastOptions={{ style: {
    background: 'hsl(var(--popover))', color: 'hsl(var(--popover-foreground))', borderColor: 'hsl(var(--border))',
    fontFamily: 'inherit', fontSize: '13px',
  } }} />;
}
