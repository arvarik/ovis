import { useEffect, useRef, useState } from 'react';
import { useNavigate, useRouterState, useSearch } from '@tanstack/react-router';
import { Search, X } from 'lucide-react';
import { cn } from '@/lib/cn';
import { Kbd } from '@/components/primitives/Kbd';
import { useIsDesktop } from '@/hooks/useMediaQuery';
import { registerSearchFocus } from '@/lib/searchFocus';

/**
 * The TopBar search surface (all viewports — F4 fix).
 *
 * Desktop on /pages: a real input. Typing filters the list live via the
 * `search` URL param (debounced 250 ms); Enter submits a content search
 * (`q`). Desktop elsewhere: navigates to /pages and focuses. Mobile: opens
 * the full-screen search sheet.
 */
export function SearchPill({ onOpenMobile }: { onOpenMobile: () => void }) {
  const isDesktop = useIsDesktop();
  const navigate = useNavigate();
  const onPages = useRouterState({
    select: (s) => s.location.pathname === '/pages' || s.location.pathname === '/pages/',
  });
  const urlSearch = useSearch({ strict: false }) as { search?: string; q?: string };
  const urlValue = urlSearch.search ?? '';
  const contentQuery = urlSearch.q;

  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(urlValue);
  const focusedRef = useRef(false);

  // Register as the `/` shortcut target while the input exists.
  const showInput = isDesktop && onPages;
  useEffect(() => {
    if (!showInput) return;
    return registerSearchFocus(() => inputRef.current?.focus());
  }, [showInput]);

  // Adopt external URL changes (presets, palette, back button) when not typing.
  useEffect(() => {
    if (!focusedRef.current) setValue(urlValue);
  }, [urlValue]);

  // Debounced live filter -> `search` param (only while in list mode).
  useEffect(() => {
    if (!showInput) return;
    const t = setTimeout(() => {
      if (value === urlValue) return;
      void navigate({
        to: '/pages',
        replace: true,
        search: (prev: Record<string, unknown>) => ({
          ...prev,
          search: value || undefined,
        }),
      });
    }, 250);
    return () => clearTimeout(t);
  }, [value, urlValue, showInput, navigate]);

  if (!isDesktop) {
    return (
      <button
        type="button"
        onClick={onOpenMobile}
        className={cn(
          'glass-pill flex h-10 w-full max-w-2xl items-center gap-2.5 rounded-full px-4',
          'text-left text-base text-ink-faint transition-colors hover:border-line-2 hover:text-ink-mute',
        )}
      >
        <Search className="size-4 shrink-0" aria-hidden />
        <span className="flex-1 truncate">
          {contentQuery ? `“${contentQuery}”` : 'Search pages…'}
        </span>
      </button>
    );
  }

  if (!showInput) {
    return (
      <button
        type="button"
        onClick={() => {
          void navigate({ to: '/pages' });
          setTimeout(() => inputRef.current?.focus(), 80);
        }}
        className={cn(
          'glass-pill flex h-9 w-full max-w-2xl items-center gap-2.5 rounded-full px-4',
          'text-left text-body text-ink-faint transition-colors hover:border-line-2 hover:text-ink-mute',
        )}
      >
        <Search className="size-4 shrink-0" aria-hidden />
        <span className="flex-1 truncate">Search pages…</span>
        <Kbd>/</Kbd>
      </button>
    );
  }

  return (
    <div
      className={cn(
        'glass-pill flex h-9 w-full max-w-2xl items-center gap-2.5 rounded-full px-4',
        'transition-colors focus-within:border-gold/60 focus-within:ring-2 focus-within:ring-gold/20',
      )}
    >
      <Search className="size-4 shrink-0 text-ink-faint" aria-hidden />
      <input
        ref={inputRef}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onFocus={() => {
          focusedRef.current = true;
        }}
        onBlur={() => {
          focusedRef.current = false;
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && value.trim() !== '') {
            // Submit = content search; the live substring filter is cleared.
            void navigate({
              to: '/pages',
              search: (prev: Record<string, unknown>) => ({
                ...prev,
                q: value.trim(),
                search: undefined,
              }),
            });
          } else if (e.key === 'Escape') {
            inputRef.current?.blur();
          }
        }}
        placeholder={contentQuery ? `results for “${contentQuery}”` : 'Filter pages… (Enter for content search)'}
        aria-label="Search pages"
        className="min-w-0 flex-1 bg-transparent text-body text-ink outline-none placeholder:text-ink-faint"
      />
      {contentQuery ? (
        <button
          type="button"
          onClick={() =>
            void navigate({
              to: '/pages',
              search: (prev: Record<string, unknown>) => ({
                ...prev,
                q: undefined,
                mode: undefined,
              }),
            })
          }
          className="flex items-center gap-1 rounded-full bg-mint/15 px-2 py-0.5 text-caption text-mint"
        >
          “{contentQuery.length > 18 ? contentQuery.slice(0, 18) + '…' : contentQuery}”
          <X className="size-3" aria-hidden />
        </button>
      ) : (
        <Kbd>/</Kbd>
      )}
    </div>
  );
}
