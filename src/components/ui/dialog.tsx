import * as React from 'react';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { MotionExit, usePopupMotion } from './motion-presence';

const DialogMotion = React.createContext<{ open: boolean; onExited?: () => void }>({ open: false });

export function Dialog({ open, children, ...props }: React.ComponentProps<typeof DialogPrimitive.Root> & { open: boolean }) {
  const onExited = React.useContext(MotionExit);
  const visible = open && !onExited;
  return <DialogMotion.Provider value={{ open: visible, onExited: onExited ?? undefined }}>
    <MotionExit.Provider value={null}>
      <DialogPrimitive.Root {...props} open={visible}>{children}</DialogPrimitive.Root>
    </MotionExit.Provider>
  </DialogMotion.Provider>;
}

export const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & {
    hideClose?: boolean;
    closeClassName?: string;
  }
>(({ className, children, hideClose, closeClassName, onEscapeKeyDown, ...props }, ref) => {
  const { open, onExited } = React.useContext(DialogMotion);
  const backdrop = React.useRef<HTMLDivElement>(null);
  const { attach, present } = usePopupMotion(open, { backdrop, onExited });
  const connect = React.useCallback((node: HTMLDivElement | null) => {
    attach(node);
    if (typeof ref === 'function') ref(node);
    else if (ref) ref.current = node;
  }, [attach, ref]);
  if (!present) return null;
  return <DialogPrimitive.Portal forceMount>
    <DialogPrimitive.Overlay ref={backdrop} className="fixed inset-0 z-50 bg-[var(--dialog-overlay)]" />
    <DialogPrimitive.Content
      ref={connect}
      {...{ inert: open ? undefined : '' }}
      onEscapeKeyDown={event => {
        if (event.target instanceof Element && event.target.closest('[data-select-open]')) event.preventDefault();
        onEscapeKeyDown?.(event);
      }}
      className={cn(
        'h1-dialog fixed left-1/2 top-1/2 z-50 grid w-[calc(100%-3rem)] max-w-lg [translate:-50%_-50%] gap-4 p-6 outline-none',
        className
      )}
      {...props}
    >
      {children}
      {!hideClose && (
        <DialogPrimitive.Close
          className={cn(
            'button-neutral absolute right-4 top-4 grid size-7 place-items-center rounded-full disabled:pointer-events-none',
            closeClassName
          )}
        >
          <X className="h-4 w-4" />
          <span className="sr-only">关闭</span>
        </DialogPrimitive.Close>
      )}
    </DialogPrimitive.Content>
  </DialogPrimitive.Portal>;
});
DialogContent.displayName = DialogPrimitive.Content.displayName;

export const DialogHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      'flex flex-col gap-1.5 text-left',
      className
    )}
    {...props}
  />
);
DialogHeader.displayName = 'DialogHeader';

export const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn(
      'text-title font-heading tracking-title',
      className
    )}
    {...props}
  />
));
DialogTitle.displayName = DialogPrimitive.Title.displayName;

export const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn('text-sm text-muted-foreground', className)}
    {...props}
  />
));
DialogDescription.displayName = DialogPrimitive.Description.displayName;
