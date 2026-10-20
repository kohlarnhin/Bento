import * as React from 'react';
import { cn } from '../../lib/utils';

interface SwitchProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'onChange'> {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}

export const Switch = React.forwardRef<HTMLButtonElement, SwitchProps>(
  ({ checked, onCheckedChange, className, onClick, ...props }, ref) => (
    <button
      {...props}
      ref={ref}
      type="button"
      role="switch"
      aria-checked={checked}
      data-state={checked ? 'checked' : 'unchecked'}
      className={cn('inline-flex h-6 w-10 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=unchecked]:bg-input', className)}
      onClick={event => { onClick?.(event); if (!event.defaultPrevented) onCheckedChange(!checked); }}
    >
      <span aria-hidden="true" className={cn('pointer-events-none block size-5 rounded-full bg-background shadow-sm transition-transform', checked ? 'translate-x-4' : 'translate-x-0')} />
    </button>
  ),
);
Switch.displayName = 'Switch';
