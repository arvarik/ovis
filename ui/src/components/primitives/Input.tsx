import { forwardRef, type InputHTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  /** Mono rendering for URL/ID fields. */
  mono?: boolean;
}

/** 16px text at base — anything smaller triggers iOS focus zoom. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, mono, ...props },
  ref,
) {
  return (
    <input
      ref={ref}
      className={cn(
        'w-full min-h-11 md:min-h-9 rounded-lg border border-line bg-well px-3 text-base md:text-body text-ink',
        'placeholder:text-ink-faint',
        'focus:border-gold/60 focus:ring-2 focus:ring-gold/20 focus:outline-none',
        'disabled:pointer-events-none disabled:opacity-50',
        mono && 'font-mono text-mono-sm',
        className,
      )}
      {...props}
    />
  );
});
