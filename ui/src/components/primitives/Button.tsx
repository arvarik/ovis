import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { cn } from '@/lib/cn';

type Variant = 'primary' | 'secondary' | 'destructive' | 'ghost';
type Size = 'md' | 'sm';

const VARIANTS: Record<Variant, string> = {
  primary: 'bg-gold text-canvas font-medium hover:bg-gold-bright active:bg-gold',
  secondary: 'border border-line-2 bg-surface text-ink hover:bg-hover active:bg-active',
  destructive: 'bg-rose/90 text-canvas font-medium hover:bg-rose',
  ghost: 'text-ink-mute hover:bg-hover hover:text-ink active:bg-active',
};

// Mobile-first: 44px touch targets at base, denser from md up.
const SIZES: Record<Size, string> = {
  md: 'min-h-11 md:min-h-9 px-4 text-body md:text-label rounded-lg',
  sm: 'min-h-11 md:min-h-8 px-3 text-label rounded-lg',
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', className, type, ...props },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? 'button'}
      className={cn(
        'inline-flex items-center justify-center gap-2 transition-colors duration-150 select-none',
        'disabled:pointer-events-none disabled:opacity-50',
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...props}
    />
  );
});

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Required: icon-only controls must name themselves. */
  label: string;
  children: ReactNode;
  variant?: 'ghost' | 'secondary';
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, className, children, variant = 'ghost', type, ...props },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? 'button'}
      aria-label={label}
      title={label}
      className={cn(
        'inline-flex items-center justify-center rounded-lg transition-colors duration-150 select-none',
        'size-11 md:size-8',
        variant === 'ghost'
          ? 'text-ink-mute hover:bg-hover hover:text-ink active:bg-active'
          : 'border border-line-2 bg-surface text-ink hover:bg-hover',
        'disabled:pointer-events-none disabled:opacity-50',
        className,
      )}
      {...props}
    >
      {children}
    </button>
  );
});
