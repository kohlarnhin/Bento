import type { HTMLAttributes } from 'react';
import { cn } from '../../lib/utils';

export function Empty({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('flex min-h-48 flex-col items-center justify-center gap-4 rounded-lg border border-dashed p-8 text-center', className)} {...props} />;
}
export function EmptyTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return <h3 className={cn('text-sm font-medium', className)} {...props} />;
}
export function EmptyDescription({ className, ...props }: HTMLAttributes<HTMLParagraphElement>) {
  return <p className={cn('max-w-sm text-xs leading-relaxed text-muted-foreground', className)} {...props} />;
}
