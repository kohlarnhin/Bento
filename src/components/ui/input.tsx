import * as React from 'react';
import { cn } from '../../lib/utils';

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input ref={ref} type={type} className={cn('h1-field flex h-9 w-full min-w-0 select-text px-3 py-2 text-[13px] outline-none transition-colors placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50', className)} {...props} />
  )
);
Input.displayName = 'Input';
