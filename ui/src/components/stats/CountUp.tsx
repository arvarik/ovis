import { useEffect, useRef, useState } from 'react';
import { count as formatCount } from '@/lib/format';

/**
 * Counts up on first appearance (600 ms, decisive ease-out). Instant when the
 * user prefers reduced motion. Formats through the shared count().
 */
export function CountUp({ value, format = formatCount }: { value: number; format?: (n: number) => string }) {
  const reduced =
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const [progress, setProgress] = useState(reduced ? 1 : 0);
  const animated = useRef(false);

  useEffect(() => {
    if (animated.current || reduced) return;
    animated.current = true;
    const start = performance.now();
    const duration = 600;
    let frame = 0;
    const tick = (t: number) => {
      const p = Math.min(1, (t - start) / duration);
      setProgress(1 - Math.pow(1 - p, 3));
      if (p < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [reduced]);

  // Progress animates 0→1 once; later `value` changes reflect instantly.
  return <>{format(Math.round(value * progress))}</>;
}
