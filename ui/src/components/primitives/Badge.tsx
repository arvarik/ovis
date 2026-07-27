import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

export type BadgeTone =
  | 'gold'
  | 'mint'
  | 'rose'
  | 'indigo'
  | 'violet'
  | 'teal'
  | 'neutral';

/** The signature idiom: bg-{accent}/15 text-{accent} border-{accent}/30. */
const TONES: Record<BadgeTone, string> = {
  gold: 'bg-gold/15 text-gold border-gold/30',
  mint: 'bg-mint/15 text-mint border-mint/30',
  rose: 'bg-rose/15 text-rose border-rose/30',
  indigo: 'bg-indigo/15 text-indigo border-indigo/30',
  violet: 'bg-violet/15 text-violet border-violet/30',
  teal: 'bg-teal/15 text-teal border-teal/30',
  neutral: 'bg-hover text-ink-mute border-line-2',
};

/**
 * Single mapping from a cc-pair / attempt status to its tone —
 * mint=ACTIVE/OK · gold=INITIAL_INDEXING/warning/parked · rose=FAILED ·
 * ink-faint=PAUSED · indigo=IN_PROGRESS.
 */
export function statusTone(status: string): BadgeTone {
  switch (status.toUpperCase()) {
    case 'ACTIVE':
    case 'SUCCESS':
    case 'OK':
      return 'mint';
    case 'INITIAL_INDEXING':
    case 'COMPLETED_WITH_ERRORS':
    case 'DEGRADED':
    case 'NOT_STARTED':
      return 'gold';
    case 'FAILED':
    case 'ERROR':
    case 'INVALID':
      return 'rose';
    case 'IN_PROGRESS':
      return 'indigo';
    case 'DELETING':
      return 'violet';
    case 'PAUSED':
    case 'CANCELED':
    default:
      return 'neutral';
  }
}

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
}

export function Badge({ tone = 'neutral', className, ...props }: BadgeProps) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-caption font-medium whitespace-nowrap',
        TONES[tone],
        className,
      )}
      {...props}
    />
  );
}
