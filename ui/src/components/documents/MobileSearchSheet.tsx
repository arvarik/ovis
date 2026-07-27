import { useState } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { Clock, Search } from 'lucide-react';
import { Sheet } from '@/components/primitives/Sheet';
import { Button } from '@/components/primitives/Button';
import { getRecentSearches, pushRecentSearch } from '@/lib/recentSearches';
import { FilterControls } from './FilterControls';
import { PresetChips } from './PresetChips';

/**
 * The mobile search surface: one thumb-reachable full-height sheet with the
 * query input on top (16px font — no iOS zoom), recent queries, preset chips
 * and the filter form below. Same URL state as the desktop pill.
 */
export function MobileSearchSheet({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const [value, setValue] = useState('');
  const recents = open ? getRecentSearches() : [];

  const submit = (q: string) => {
    const trimmed = q.trim();
    if (trimmed === '') return;
    pushRecentSearch(trimmed);
    onOpenChange(false);
    setValue('');
    void navigate({
      to: '/pages',
      search: (prev: Record<string, unknown>) => ({ ...prev, q: trimmed, search: undefined }),
    });
  };

  return (
    <Sheet
      open={open}
      onOpenChange={onOpenChange}
      title="Search"
      description="Search pages and adjust filters"
      contentClassName="h-[94dvh]"
    >
      <div className="flex min-h-0 flex-1 flex-col">
        <form
          className="flex items-center gap-2 border-b border-line px-4 py-3"
          onSubmit={(e) => {
            e.preventDefault();
            submit(value);
          }}
        >
          <Search className="size-4 shrink-0 text-ink-faint" aria-hidden />
          <input
            value={value}
            onChange={(e) => setValue(e.target.value)}
            autoFocus
            placeholder="Search page content…"
            aria-label="Search page content"
            className="min-w-0 flex-1 bg-transparent text-base text-ink outline-none placeholder:text-ink-faint"
          />
          <Button type="submit" variant="primary" size="sm" disabled={value.trim() === ''}>
            Search
          </Button>
        </form>

        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto p-4 pb-[max(env(safe-area-inset-bottom),1rem)]">
          {recents.length > 0 ? (
            <section aria-label="Recent searches" className="space-y-1">
              {recents.map((r) => (
                <button
                  key={r}
                  type="button"
                  onClick={() => submit(r)}
                  className="flex min-h-11 w-full items-center gap-3 rounded-lg px-2 text-left text-body text-ink-mute transition-colors hover:bg-hover hover:text-ink"
                >
                  <Clock className="size-4 shrink-0 text-ink-faint" aria-hidden />
                  <span className="truncate">{r}</span>
                </button>
              ))}
            </section>
          ) : null}

          <section aria-label="Presets">
            <PresetChips />
          </section>

          <section aria-label="Filters">
            <FilterControls />
          </section>
        </div>
      </div>
    </Sheet>
  );
}
