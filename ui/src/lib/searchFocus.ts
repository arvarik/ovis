/**
 * Focus registry for the `/` shortcut: the Explorer's search input registers
 * itself; `focusSearch()` reports whether anything took the focus, so the
 * caller can fall back (e.g. open the palette on non-Explorer routes).
 */
let target: (() => void) | null = null;

export function registerSearchFocus(fn: () => void): () => void {
  target = fn;
  return () => {
    if (target === fn) target = null;
  };
}

export function focusSearch(): boolean {
  if (!target) return false;
  target();
  return true;
}
