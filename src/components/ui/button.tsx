import * as React from 'react';
import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '../../lib/utils';

export const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-full text-[length:var(--text-control)] font-medium outline-none transition-[color,background-color,box-shadow] focus-visible:ring-2 focus-visible:ring-ring/25 disabled:pointer-events-none disabled:opacity-50 select-none cursor-pointer [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        default:
          'bg-primary text-primary-foreground hover:bg-[var(--primary-hover)]',
        destructive:
          'bg-primary text-primary-foreground hover:bg-[var(--primary-hover)]',
        outline: 'button-neutral',
        secondary: 'button-neutral',
        ghost: 'button-neutral',
        link:
          'text-primary underline-offset-4 hover:underline',
        'ghost-destructive': 'button-neutral',
        terminal: 'button-neutral',
      },
      size: {
        default: 'h-9 px-4 py-2',
        sm: 'h-8 px-3 [&_svg]:size-3.5',
        lg: 'h-10 px-6',
        icon: 'size-8 p-0',
        'icon-sm': 'size-7 p-0 [&_svg]:size-3.5',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  }
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button';
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  }
);
Button.displayName = 'Button';

export default Button;
