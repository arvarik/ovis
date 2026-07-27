import { useEffect, useState } from 'react';
import { count as formatCount } from '@/lib/format';

/**
 * Counts up on first appearance (600 ms, decisive ease-out). Instant when the
 * user prefers reduced motion. The animation drives a 0→1 progress that the
 * current value multiplies through, so later value changes render instantly —
 * and an interrupted effect (StrictMode remount) simply restarts it.
 */
export function CountUp({
  value,
  format = formatCount,
}: {
  value: number;
  format?: (n: number) => string;
}) {
  const reduced =
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const [progress, setProgress] = useState(reduced ? 1 : 0);

  useEffect(() => {
    if (reduced) return;
    let frame = 0;
    const start = performance.now();
    const duration = 600;
    const tick = (t: number) => {
      const p = Math.min(1, (t - start) / duration);
      setProgress(1 - Math.pow(1 - p, 3));
      if (p < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [reduced]);

  return <>{format(Math.round(value * progress))}</>;
}
