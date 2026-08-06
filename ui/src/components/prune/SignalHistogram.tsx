/**
 * The measured distribution behind a threshold.
 *
 * The whole point of the v2 profile layer is that a threshold is a review-time
 * decision made against real measurements rather than a number guessed in a
 * config file — and a number with no distribution behind it is still a guess.
 * So the dial draws what the corpus actually looks like, with the two band
 * edges marked on it, and says how many documents sit above each.
 *
 * Hand-drawn rather than charted: this is a strip of bars and two rules, the
 * chart library is deliberately confined to the lazily-loaded stats route, and
 * pulling it in here would put it in the initial bundle for everyone.
 */
import { useMemo } from 'react';
import type { PruneHistogramBucket } from '@/api/types';
import { count as formatCount } from '@/lib/format';

const HEIGHT = 56;

export function SignalHistogram({
  buckets,
  auto,
  review,
  label,
  pending,
}: {
  buckets: PruneHistogramBucket[];
  /** Bulk-band edge, if the policy sets one. */
  auto: number | null;
  /** Review-band edge, if the policy sets one. */
  review: number | null;
  /** What one bar counts, for the accessible description. */
  label: string;
  pending?: boolean;
}) {
  const { max, lo, hi, total, aboveAuto, aboveReview } = useMemo(() => {
    const max = buckets.reduce((m, b) => Math.max(m, b.count), 0);
    const lo = buckets.at(0)?.lower ?? 0;
    const hi = buckets.at(-1)?.upper ?? 1;
    const total = buckets.reduce((sum, b) => sum + b.count, 0);
    // A bucket counts toward a threshold when its *lower* edge clears it: the
    // documents in it are all at least that similar.
    const above = (t: number | null) =>
      t === null ? null : buckets.filter((b) => b.lower >= t).reduce((s, b) => s + b.count, 0);
    return { max, lo, hi, total, aboveAuto: above(auto), aboveReview: above(review) };
  }, [buckets, auto, review]);

  if (pending) {
    return <div className="h-14 animate-pulse rounded-md bg-well" />;
  }

  // Nothing measured is a fact worth stating: a flat empty strip reads as
  // "everything is zero" when it means "no scan has looked at this yet".
  if (buckets.length === 0 || total === 0) {
    return (
      <div className="flex h-14 items-center justify-center rounded-md border border-dashed border-line text-caption text-ink-mute">
        Nothing measured for this signal yet — run a scan that computes it.
      </div>
    );
  }

  const span = hi - lo || 1;
  const position = (value: number) => ((value - lo) / span) * 100;

  return (
    <div>
      <div
        className="relative flex h-14 items-end gap-px overflow-hidden rounded-md bg-well px-px"
        role="img"
        aria-label={`Distribution of ${label} across ${formatCount(total)} measured documents`}
      >
        {buckets.map((bucket) => {
          const inAuto = auto !== null && bucket.lower >= auto;
          const inReview = !inAuto && review !== null && bucket.lower >= review;
          return (
            <div
              key={`${bucket.lower}-${bucket.upper}`}
              className={
                inAuto
                  ? 'flex-1 rounded-t-[1px] bg-rose/70'
                  : inReview
                    ? 'flex-1 rounded-t-[1px] bg-gold/70'
                    : 'flex-1 rounded-t-[1px] bg-ink-faint/30'
              }
              style={{ height: `${Math.max((bucket.count / max) * HEIGHT, 1)}px` }}
              title={`${bucket.lower.toFixed(2)}–${bucket.upper.toFixed(2)}: ${formatCount(bucket.count)}`}
            />
          );
        })}
        {review !== null ? <Marker at={position(review)} tone="gold" /> : null}
        {auto !== null ? <Marker at={position(auto)} tone="rose" /> : null}
      </div>

      <div className="mt-1 flex flex-wrap justify-between gap-x-3 text-caption text-ink-mute">
        <span className="tabular-nums">{lo.toFixed(2)}</span>
        <span>
          {aboveReview !== null ? (
            <span className="text-gold">{formatCount(aboveReview)} above review</span>
          ) : null}
          {aboveReview !== null && aboveAuto !== null ? ' · ' : ''}
          {aboveAuto !== null ? (
            <span className="text-rose">{formatCount(aboveAuto)} above bulk</span>
          ) : null}
        </span>
        <span className="tabular-nums">{hi.toFixed(2)}</span>
      </div>
    </div>
  );
}

function Marker({ at, tone }: { at: number; tone: 'gold' | 'rose' }) {
  return (
    <span
      aria-hidden
      className={`absolute top-0 bottom-0 w-px ${tone === 'rose' ? 'bg-rose' : 'bg-gold'}`}
      style={{ left: `${Math.min(Math.max(at, 0), 100)}%` }}
    />
  );
}
