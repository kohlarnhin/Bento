import * as React from 'react';
import { cn } from '../../lib/utils';

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input ref={ref} type={type} className={cn('flex h-9 w-full min-w-0 select-text rounded-md border border-input bg-background px-3 py-2 text-[13px] shadow-sm placeholder:text-muted-foreground outline-none transition-[color,border-color] focus-visible:border-foreground/60 disabled:cursor-not-allowed disabled:opacity-50 aria-[invalid=true]:border-destructive aria-[invalid=true]:focus-visible:border-destructive', className)} {...props} />
  )
);
Input.displayName = 'Input';
