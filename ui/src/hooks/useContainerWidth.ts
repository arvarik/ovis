import { useEffect, useRef, useState } from 'react';

/**
 * Container-driven responsiveness (P5): the CONTAINER, not the viewport,
 * decides the list's shape — so the list also degrades gracefully when a
 * side panel splits the screen.
 */
export function useContainerWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w !== undefined) setWidth(w);
    });
    observer.observe(el);
    setWidth(el.clientWidth);
    return () => observer.disconnect();
  }, []);

  return [ref, width] as const;
}
