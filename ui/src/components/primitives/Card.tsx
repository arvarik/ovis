import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

/** Level-1 surface: bg-surface + border-line, rounded-xl. */
export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn('rounded-xl border border-line bg-surface p-4', className)}
      {...props}
    />
  );
}
