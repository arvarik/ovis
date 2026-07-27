import { useEffect, useState, type HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

/**
 * Content-shaped shimmer block. Renders nothing for the first 150 ms so fast
 * responses never flash a skeleton.
 */
export function Skeleton({
  className,
  delayMs = 150,
  ...props
}: HTMLAttributes<HTMLDivElement> & { delayMs?: number }) {
  const [visible, setVisible] = useState(delayMs === 0);
  useEffect(() => {
    if (delayMs === 0) return;
    const t = setTimeout(() => setVisible(true), delayMs);
    return () => clearTimeout(t);
  }, [delayMs]);

  return (
    <div
      aria-hidden
      className={cn(
        'rounded-lg bg-hover',
        visible ? 'animate-pulse' : 'invisible',
        className,
      )}
      {...props}
    />
  );
}
