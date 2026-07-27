import { Loader2 } from 'lucide-react';
import { cn } from '@/lib/cn';

export function Spinner({ className, label }: { className?: string; label?: string }) {
  return (
    <span role="status" aria-label={label ?? 'Loading'} className="inline-flex items-center gap-2">
      <Loader2 className={cn('size-4 animate-spin text-ink-mute', className)} aria-hidden />
      {label ? <span className="text-label text-ink-mute">{label}</span> : null}
    </span>
  );
}
