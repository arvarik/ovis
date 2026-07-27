import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

export function Kbd({ className, ...props }: HTMLAttributes<HTMLElement>) {
  return (
    <kbd
      className={cn(
        'inline-flex min-w-5 items-center justify-center rounded-md border border-line-3 bg-well px-1.5 py-0.5',
        'font-mono text-caption text-ink-mute',
        className,
      )}
      {...props}
    />
  );
}
