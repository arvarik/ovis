import { useNavigate, useSearch } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { presetCountQuery } from '@/api/queries';
import type { QueryParams } from '@/api/client';
import { cn } from '@/lib/cn';
import { compact } from '@/lib/format';
import type { PagesSearch } from '@/routes/pages';

/**
 * Server-backed presets (D5 fix): each chip is a canned URL search-param set,
 * counts are global truths scoped to the active connector/source/search
 * filters — or absent while loading. Never computed over the visible page.
 */

type PresetKey = 'all' | 'stubs' | 'heavy' | 'recent' | 'hidden';

/** Hour-aligned so the query key (and count cache) stays stable. */
function recentCutoff(): string {
  const HOUR = 3_600_000;
  return new Date(Math.floor(Date.now() / HOUR) * HOUR - 24 * HOUR).toISOString();
}

interface PresetDef {
  key: PresetKey;
  label: string;
  params: () => Partial<PagesSearch>;
}

/** The param fields presets own — applying a preset clears the others. */
const PRESET_FIELDS: (keyof PagesSearch)[] = ['chunk_min', 'chunk_max', 'hidden', 'updated_after'];

const PRESETS: PresetDef[] = [
  { key: 'all', label: 'All', params: () => ({}) },
  { key: 'stubs', label: 'Stubs', params: () => ({ chunk_min: 0, chunk_max: 0 }) },
  { key: 'heavy', label: 'Heavy', params: () => ({ chunk_min: 11 }) },
  { key: 'recent', label: 'Recent', params: () => ({ updated_after: recentCutoff() }) },
  { key: 'hidden', label: 'Hidden', params: () => ({ hidden: true }) },
];

export function activePreset(search: PagesSearch): PresetKey | null {
  if (search.chunk_min === 0 && search.chunk_max === 0) return 'stubs';
  if (search.chunk_min === 11 && search.chunk_max === undefined) return 'heavy';
  if (search.hidden === true && search.chunk_min === undefined) return 'hidden';
  if (search.updated_after !== undefined && search.chunk_min === undefined) return 'recent';
  const anyPresetField = PRESET_FIELDS.some((f) => search[f] !== undefined);
  return anyPresetField ? null : 'all';
}

function Chip({ def, search }: { def: PresetDef; search: PagesSearch }) {
  const navigate = useNavigate();
  const active = activePreset(search) === def.key;

  // Count scope: the preset's own params + the user's connector/source/search
  // filters. Global truth for the current context, never page-local.
  const params: QueryParams = {
    connector_id: search.connector,
    source: search.source,
    search: search.search,
    ...Object.fromEntries(
      Object.entries(def.params()).map(([k, v]) => [k === 'connector' ? 'connector_id' : k, v]),
    ),
  };
  const count = useQuery(presetCountQuery(params));

  const apply = () => {
    void navigate({
      to: '/pages',
      search: (prev) => {
        const next: PagesSearch = { ...(prev as PagesSearch) };
        for (const f of PRESET_FIELDS) delete next[f];
        return { ...next, ...def.params() };
      },
    });
  };

  return (
    <button
      type="button"
      onClick={apply}
      aria-pressed={active}
      className={cn(
        'flex min-h-11 shrink-0 snap-start items-center gap-1.5 rounded-full border px-3.5 text-label transition-colors md:min-h-8',
        active
          ? 'border-gold/40 bg-gold/15 text-gold'
          : 'border-line bg-surface text-ink-mute hover:bg-hover hover:text-ink',
      )}
    >
      {def.label}
      {count.data ? (
        <span className={cn('font-mono text-caption', active ? 'text-gold/80' : 'text-ink-faint')}>
          {count.data.exact ? '' : '~'}
          {compact(count.data.total)}
        </span>
      ) : null}
    </button>
  );
}

export function PresetChips() {
  // Non-strict: this also renders inside the shell-level mobile search sheet,
  // which can be opened from any route.
  const search = useSearch({ strict: false }) as PagesSearch;
  return (
    <div
      role="group"
      aria-label="Presets"
      className="flex snap-x items-center gap-2 overflow-x-auto pb-0.5 [scrollbar-width:none]"
    >
      {PRESETS.map((def) => (
        <Chip key={def.key} def={def} search={search} />
      ))}
    </div>
  );
}
