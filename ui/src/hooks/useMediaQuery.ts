import { useMemo, useSyncExternalStore } from 'react';

export function useMediaQuery(query: string): boolean {
  const [subscribe, getSnapshot] = useMemo(() => {
    const mql = window.matchMedia(query);
    return [
      (onChange: () => void) => {
        mql.addEventListener('change', onChange);
        return () => mql.removeEventListener('change', onChange);
      },
      () => mql.matches,
    ] as const;
  }, [query]);
  return useSyncExternalStore(subscribe, getSnapshot);
}

/** The single breakpoint the Sheet primitive keys on: md = 768px. */
export function useIsDesktop(): boolean {
  return useMediaQuery('(min-width: 768px)');
}
