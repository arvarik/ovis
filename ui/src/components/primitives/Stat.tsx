import type { ReactNode } from 'react';
import { cn } from '@/lib/cn';
import { Card } from './Card';

/**
 * Stat tile: serif numerals over mono captions — the signature look.
 * `approximate` renders the ~ prefix for planner estimates (total_exact: false).
 */
export function Stat({
  label,
  value,
  caption,
  approximate,
  tone,
  onClick,
  className,
}: {
  label: string;
  value: ReactNode;
  caption?: ReactNode;
  approximate?: boolean;
  tone?: 'gold' | 'mint' | 'rose' | 'default';
  onClick?: () => void;
  className?: string;
}) {
  const toneClass =
    tone === 'gold'
      ? 'text-gold'
      : tone === 'mint'
        ? 'text-mint'
        : tone === 'rose'
          ? 'text-rose'
          : 'text-ink';

  const body = (
    <>
      <div className="text-label text-ink-mute">{label}</div>
      <div className={cn('stat-numeral text-display-xl leading-none', toneClass)}>
        {approximate ? <span className="text-ink-faint">~</span> : null}
        {value}
      </div>
      {caption ? <div className="font-mono text-caption text-ink-faint">{caption}</div> : null}
    </>
  );

  if (onClick) {
    return (
      <Card
        role="button"
        tabIndex={0}
        onClick={onClick}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onClick();
          }
        }}
        className={cn(
          'flex cursor-pointer flex-col gap-1.5 transition-colors hover:bg-hover',
          className,
        )}
      >
        {body}
      </Card>
    );
  }

  return <Card className={cn('flex flex-col gap-1.5', className)}>{body}</Card>;
}
