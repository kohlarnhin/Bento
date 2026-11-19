import type { HTMLAttributes } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '../../lib/utils';

const alertVariants = cva('h1-group relative w-full p-4 text-sm [&>svg]:absolute [&>svg]:left-4 [&>svg]:top-4 [&>svg]:size-4 [&>svg~*]:pl-7', {
  variants: {
    variant: {
      default: 'text-foreground',
      destructive: '[&>svg]:text-destructive',
      warning: '[&>svg]:text-warning-foreground',
    },
  },
  defaultVariants: { variant: 'default' },
});
export function Alert({ className, variant, ...props }: HTMLAttributes<HTMLDivElement> & VariantProps<typeof alertVariants>) {
  return <div role="alert" className={cn(alertVariants({ variant }), className)} {...props} />;
}
export function AlertTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return <h3 className={cn('mb-1 font-medium leading-none tracking-tight', className)} {...props} />;
}
export function AlertDescription({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('text-xs leading-relaxed', className)} {...props} />;
}
