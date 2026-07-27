import { useState, type ReactNode, type SelectHTMLAttributes } from 'react';
import { useNavigate, useSearch } from '@tanstack/react-router';
import { Popover } from 'radix-ui';
import { SlidersHorizontal, X } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { connectorsQuery } from '@/api/queries';
import { cn } from '@/lib/cn';
import { compact, sourceLabel } from '@/lib/format';
import { Button } from '@/components/primitives/Button';
import { Input } from '@/components/primitives/Input';
import { Sheet } from '@/components/primitives/Sheet';
import { useIsDesktop } from '@/hooks/useMediaQuery';
import { PAGE_SORTS, type PagesSearch, type PagesSort } from '@/routes/pages';

const SORT_LABELS: Record<PagesSort, string> = {
  updated_desc: 'Updated — newest first',
  updated_asc: 'Updated — oldest first',
  chunks_desc: 'Chunks — most first',
  chunks_asc: 'Chunks — fewest first',
  id_asc: 'URL — A to Z',
  id_desc: 'URL — Z to A',
  boost_desc: 'Boost — highest first',
};

export function useUpdatePagesSearch() {
  const navigate = useNavigate();
  return (patch: Partial<PagesSearch>, replace = false) =>
    void navigate({
      to: '/pages',
      replace,
      // prev is the cross-route search union; on /pages it is PagesSearch.
      search: (prev) => ({ ...(prev as PagesSearch), ...patch }),
    });
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-label text-ink-mute">{label}</span>
      {children}
    </label>
  );
}

function Select({ className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={cn(
        'min-h-11 w-full rounded-lg border border-line bg-well px-3 text-base text-ink md:min-h-9 md:text-body',
        'focus:border-gold/60 focus:ring-2 focus:ring-gold/20 focus:outline-none',
        className,
      )}
      {...props}
    />
  );
}

/** How many filters are active (shown on the filter button badge). */
export function activeFilterCount(search: PagesSearch): number {
  let n = 0;
  if (search.connector !== undefined) n++;
  if (search.source !== undefined) n++;
  if (search.hidden !== undefined) n++;
  if (search.chunk_min !== undefined || search.chunk_max !== undefined) n++;
  if (search.updated_after !== undefined || search.updated_before !== undefined) n++;
  return n;
}

/**
 * The one filter form — rendered inside a Radix popover on desktop and the
 * mobile search sheet / filter sheet on phones. Writes straight to the URL.
 */
export function FilterControls() {
  // Non-strict: also rendered from the shell-level mobile search sheet.
  const search = useSearch({ strict: false }) as PagesSearch;
  const update = useUpdatePagesSearch();
  const connectors = useQuery(connectorsQuery);

  const sources = [...new Set((connectors.data ?? []).map((c) => c.source))].sort();
  const sorted = (connectors.data ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div className="flex flex-col gap-4">
      <Field label="Connector">
        <Select
          value={search.connector ?? ''}
          onChange={(e) =>
            update({ connector: e.target.value === '' ? undefined : Number(e.target.value) })
          }
        >
          <option value="">Any connector</option>
          {sorted.map((c) => (
            <option key={c.cc_pair_id} value={c.connector_id}>
              {c.name} ({compact(c.doc_count)})
            </option>
          ))}
        </Select>
      </Field>

      <Field label="Source">
        <Select
          value={search.source ?? ''}
          onChange={(e) => update({ source: e.target.value === '' ? undefined : e.target.value })}
        >
          <option value="">Any source</option>
          {sources.map((s) => (
            <option key={s} value={s.toLowerCase()}>
              {sourceLabel(s)}
            </option>
          ))}
        </Select>
      </Field>

      <Field label="Visibility">
        <Select
          value={search.hidden === undefined ? '' : String(search.hidden)}
          onChange={(e) =>
            update({ hidden: e.target.value === '' ? undefined : e.target.value === 'true' })
          }
        >
          <option value="">All pages</option>
          <option value="false">Visible only</option>
          <option value="true">Hidden only</option>
        </Select>
      </Field>

      <div className="grid grid-cols-2 gap-3">
        <Field label="Chunks ≥">
          <Input
            type="number"
            min={0}
            inputMode="numeric"
            defaultValue={search.chunk_min ?? ''}
            key={`min-${search.chunk_min ?? ''}`}
            onBlur={(e) =>
              update({ chunk_min: e.target.value === '' ? undefined : Number(e.target.value) })
            }
            placeholder="any"
          />
        </Field>
        <Field label="Chunks ≤">
          <Input
            type="number"
            min={0}
            inputMode="numeric"
            defaultValue={search.chunk_max ?? ''}
            key={`max-${search.chunk_max ?? ''}`}
            onBlur={(e) =>
              update({ chunk_max: e.target.value === '' ? undefined : Number(e.target.value) })
            }
            placeholder="any"
          />
        </Field>
      </div>

      <Field label="Sort">
        <Select
          value={search.sort ?? 'updated_desc'}
          onChange={(e) => {
            const v = e.target.value as PagesSort;
            update({ sort: v === 'updated_desc' ? undefined : v });
          }}
        >
          {PAGE_SORTS.map((s) => (
            <option key={s} value={s}>
              {SORT_LABELS[s]}
            </option>
          ))}
        </Select>
      </Field>

      {activeFilterCount(search) > 0 ? (
        <Button
          variant="ghost"
          onClick={() =>
            update({
              connector: undefined,
              source: undefined,
              hidden: undefined,
              chunk_min: undefined,
              chunk_max: undefined,
              updated_after: undefined,
              updated_before: undefined,
            })
          }
        >
          Clear all filters
        </Button>
      ) : null}
    </div>
  );
}

/** Filter trigger: Radix popover ≥md, bottom sheet below. Same form inside. */
export function FilterButton() {
  const isDesktop = useIsDesktop();
  const search = useSearch({ strict: false }) as PagesSearch;
  const [open, setOpen] = useState(false);
  const count = activeFilterCount(search);

  const trigger = (
    <button
      type="button"
      onClick={isDesktop ? undefined : () => setOpen(true)}
      className={cn(
        'flex min-h-11 shrink-0 items-center gap-2 rounded-full border px-3.5 text-label transition-colors md:min-h-8',
        count > 0
          ? 'border-gold/40 bg-gold/10 text-gold'
          : 'border-line bg-surface text-ink-mute hover:bg-hover hover:text-ink',
      )}
    >
      <SlidersHorizontal className="size-4" aria-hidden />
      Filters
      {count > 0 ? <span className="font-mono text-caption">{count}</span> : null}
    </button>
  );

  if (!isDesktop) {
    return (
      <>
        {trigger}
        <Sheet open={open} onOpenChange={setOpen} title="Filters">
          <div className="flex items-center justify-between px-5 pt-2">
            <h2 className="font-display font-display-soft text-title text-ink">Filters</h2>
            <button
              type="button"
              onClick={() => setOpen(false)}
              aria-label="Close filters"
              className="flex size-11 items-center justify-center rounded-lg text-ink-mute"
            >
              <X className="size-5" aria-hidden />
            </button>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-5 pt-3 pb-[max(env(safe-area-inset-bottom),1.25rem)]">
            <FilterControls />
          </div>
        </Sheet>
      </>
    );
  }

  return (
    <Popover.Root>
      <Popover.Trigger asChild>{trigger}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={8}
          collisionPadding={12}
          className="glass-panel z-40 w-96 max-w-[calc(100vw-24px)] rounded-xl p-4 animate-scale-in"
        >
          <FilterControls />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
