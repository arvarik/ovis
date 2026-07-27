import { useCallback, useMemo, useRef, useState } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { ArrowUp, Eye, EyeOff, Radio, SearchX, Trash2, X } from 'lucide-react';
import { pagesInfiniteQuery, searchQuery } from '@/api/queries';
import { useBatchDelete, useHidePages } from '@/api/mutations';
import type { PageListItem, SearchHit } from '@/api/types';
import { cn } from '@/lib/cn';
import { count as formatCount, sourceLabel } from '@/lib/format';
import { Button } from '@/components/primitives/Button';
import { AlertDialog } from '@/components/primitives/Dialog';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { MenuItem } from '@/components/primitives/Menu';
import { Skeleton } from '@/components/primitives/Skeleton';
import { useHotkeys } from '@/hooks/hotkeys';
import { pagesRoute, SEARCH_MODES, type PagesSearch } from '@/routes/pages';
import { DocumentList, type ExplorerRow } from './DocumentList';
import { FilterButton, useUpdatePagesSearch } from './FilterControls';
import { PresetChips } from './PresetChips';
import { SelectionBar } from './SelectionBar';
import { useLivePages } from './useLivePages';

function pageToRow(p: PageListItem): ExplorerRow {
  return {
    id: p.id,
    title: p.semantic_id,
    link: p.link,
    connectorId: p.connector_id,
    connectorName: p.connector_name,
    source: p.connector_source,
    chunkCount: p.chunk_count,
    updatedAt: p.updated_at,
    hidden: p.hidden,
    boost: p.boost,
  };
}

function hitToRow(h: SearchHit): ExplorerRow {
  return {
    id: h.document_id,
    title: h.semantic_id ?? h.document_id,
    link: h.link,
    connectorId: h.connector_id,
    connectorName: h.connector_name,
    source: h.connector_source,
    chunkCount: h.chunk_count,
    updatedAt: h.updated_at,
    hidden: false,
    boost: 0,
    score: h.score,
    snippet: h.snippet,
  };
}

/** Open string — known values get plain English, the rest render verbatim. */
function explainDegraded(value: string): string {
  switch (value) {
    case 'no_knn_field':
      return 'semantic unavailable on this index — keyword results';
    case 'no_embedder':
      return 'embedder unavailable — keyword results';
    case 'connector_filter_post_applied':
      return 'connector filter applied after ranking — results may be incomplete';
    default:
      return value;
  }
}

/** Human recap of active filters for empty states. */
function filterRecap(search: PagesSearch): string {
  const parts: string[] = [];
  if (search.search) parts.push(`title contains “${search.search}”`);
  if (search.connector !== undefined) parts.push(`connector #${search.connector}`);
  if (search.source) parts.push(`source ${sourceLabel(search.source)}`);
  if (search.hidden !== undefined) parts.push(search.hidden ? 'hidden only' : 'visible only');
  if (search.chunk_min !== undefined || search.chunk_max !== undefined)
    parts.push(
      `chunks ${search.chunk_min ?? 0}–${search.chunk_max ?? '∞'}`,
    );
  if (search.updated_after) parts.push('recently updated');
  return parts.join(' · ');
}

export function PagesView() {
  const search = pagesRoute.useSearch();
  const navigate = useNavigate();
  const update = useUpdatePagesSearch();

  const isSearchMode = (search.q ?? '') !== '';
  const liveWanted = search.live === true && !isSearchMode;
  const [liveFailed, setLiveFailed] = useState(false);
  const liveActive = liveWanted && !liveFailed;

  const listQ = useInfiniteQuery({
    ...pagesInfiniteQuery(search),
    enabled: !isSearchMode && !liveActive,
  });
  const contentQ = useQuery(searchQuery(search));
  const live = useLivePages(search, liveActive, () => setLiveFailed(true));

  const rows: ExplorerRow[] = useMemo(() => {
    if (isSearchMode) return (contentQ.data?.items ?? []).map(hitToRow);
    if (liveActive) return live.rows.map(pageToRow);
    return (listQ.data?.pages ?? []).flatMap((p) => p.items).map(pageToRow);
  }, [isSearchMode, liveActive, contentQ.data, live.rows, listQ.data]);

  // ----- selection -------------------------------------------------------
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const lastToggled = useRef<string | null>(null);
  const toggleSelect = useCallback(
    (id: string, range: boolean) => {
      setSelected((prev) => {
        const next = new Set(prev);
        if (range && lastToggled.current) {
          const a = rows.findIndex((r) => r.id === lastToggled.current);
          const b = rows.findIndex((r) => r.id === id);
          if (a !== -1 && b !== -1) {
            for (let i = Math.min(a, b); i <= Math.max(a, b); i++) {
              next.add(rows[i]!.id);
            }
            return next;
          }
        }
        if (next.has(id)) next.delete(id);
        else next.add(id);
        lastToggled.current = id;
        return next;
      });
    },
    [rows],
  );
  const clearSelection = useCallback(() => setSelected(new Set()), []);

  // ----- keyboard --------------------------------------------------------
  const [activeIndex, setActiveIndex] = useState(0);
  const listApi = useRef<{ scrollToIndex: (i: number) => void } | null>(null);
  const clampedActive = Math.min(activeIndex, Math.max(rows.length - 1, 0));

  const moveActive = useCallback(
    (delta: number) => {
      setActiveIndex((prev) => {
        const next = Math.max(0, Math.min(rows.length - 1, prev + delta));
        listApi.current?.scrollToIndex(next);
        return next;
      });
    },
    [rows.length],
  );

  const inspect = useCallback(
    (row: ExplorerRow) => {
      void navigate({
        to: '/pages/$docId',
        params: { docId: row.id },
        search: (prev: Record<string, unknown>) => prev,
      });
    },
    [navigate],
  );

  // ----- mutations -------------------------------------------------------
  const batchDelete = useBatchDelete();
  const hidePages = useHidePages();
  const [deleteIds, setDeleteIds] = useState<string[] | null>(null);

  const confirmDelete = () => {
    if (!deleteIds || deleteIds.length === 0) return;
    batchDelete.mutate(deleteIds, {
      onSettled: () => {
        setDeleteIds(null);
        setSelected((prev) => {
          const next = new Set(prev);
          for (const id of deleteIds) next.delete(id);
          return next;
        });
      },
    });
  };

  useHotkeys(
    [
      { keys: 'j', description: 'Next row', group: 'Explorer', scope: 'route', handler: () => moveActive(1) },
      { keys: 'k', description: 'Previous row', group: 'Explorer', scope: 'route', handler: () => moveActive(-1) },
      { keys: 'arrowdown', description: '', group: 'Explorer', scope: 'route', hidden: true, handler: () => moveActive(1) },
      { keys: 'arrowup', description: '', group: 'Explorer', scope: 'route', hidden: true, handler: () => moveActive(-1) },
      {
        keys: 'enter',
        description: 'Inspect active row',
        group: 'Explorer',
        scope: 'route',
        handler: () => {
          const row = rows[clampedActive];
          if (row) inspect(row);
        },
      },
      {
        keys: 'o',
        description: 'Open link in new tab',
        group: 'Explorer',
        scope: 'route',
        handler: () => {
          const row = rows[clampedActive];
          if (row?.link) window.open(row.link, '_blank', 'noopener');
        },
      },
      {
        keys: 'x',
        description: 'Select active row',
        group: 'Explorer',
        scope: 'route',
        handler: () => {
          const row = rows[clampedActive];
          if (row) toggleSelect(row.id, false);
        },
      },
      {
        keys: 'shift+x',
        description: 'Select range to active row',
        group: 'Explorer',
        scope: 'route',
        handler: () => {
          const row = rows[clampedActive];
          if (row) toggleSelect(row.id, true);
        },
      },
      {
        keys: 'escape',
        description: 'Clear selection',
        group: 'Explorer',
        scope: 'route',
        hidden: true,
        handler: () => clearSelection(),
      },
    ],
    rows.length > 0,
  );

  // ----- derived totals --------------------------------------------------
  const firstPage = listQ.data?.pages[0];
  const total = isSearchMode
    ? contentQ.data?.total_hits
    : liveActive
      ? live.done?.total_matched
      : firstPage?.total;
  const totalExact = isSearchMode
    ? (contentQ.data?.total_hits_exact ?? true)
    : liveActive
      ? true
      : (firstPage?.total_exact ?? true);

  const currentError = isSearchMode ? contentQ.error : liveActive ? null : listQ.error;
  const isLoading = isSearchMode
    ? contentQ.isPending
    : liveActive
      ? live.phase === 'streaming' && live.rows.length === 0
      : listQ.isPending;

  const liveCapped =
    liveActive && live.phase === 'done' && live.done !== null && live.done.total_matched > live.rows.length;

  const recap = filterRecap(search);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 space-y-2 px-3 pt-3 md:px-4">
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <PresetChips />
          </div>
          <FilterButton />
          <button
            type="button"
            aria-pressed={liveWanted}
            onClick={() => {
              setLiveFailed(false);
              update({ live: search.live ? undefined : true });
            }}
            className={cn(
              'flex min-h-11 shrink-0 items-center gap-1.5 rounded-full border px-3.5 text-label transition-colors md:min-h-8',
              liveWanted
                ? liveFailed
                  ? 'border-gold/40 bg-gold/10 text-gold'
                  : 'border-mint/40 bg-mint/10 text-mint'
                : 'border-line bg-surface text-ink-mute hover:bg-hover hover:text-ink',
            )}
            title={isSearchMode ? 'Live mode applies to the list, not content search' : undefined}
          >
            <Radio
              className={cn('size-4', liveActive && live.phase === 'streaming' && 'animate-pulse-dot')}
              aria-hidden
            />
            {liveWanted ? (liveFailed ? 'live unavailable — polling' : 'LIVE') : 'Live'}
          </button>
        </div>

        {isSearchMode ? (
          <div className="flex flex-wrap items-center gap-2">
            <div role="group" aria-label="Search mode" className="flex items-center gap-1">
              {SEARCH_MODES.map((m) => {
                const active = (search.mode ?? 'keyword') === m;
                return (
                  <button
                    key={m}
                    type="button"
                    aria-pressed={active}
                    onClick={() => update({ mode: m === 'keyword' ? undefined : m })}
                    className={cn(
                      'min-h-11 rounded-full border px-3 text-label transition-colors md:min-h-7',
                      active
                        ? 'border-gold/40 bg-gold/15 text-gold'
                        : 'border-line bg-surface text-ink-mute hover:bg-hover',
                    )}
                  >
                    {m}
                  </button>
                );
              })}
            </div>
            {contentQ.data?.degraded ? (
              <span className="flex items-center gap-1.5 rounded-full border border-gold/30 bg-gold/10 px-3 py-1 text-caption text-gold">
                {explainDegraded(contentQ.data.degraded)}
              </span>
            ) : null}
            <button
              type="button"
              onClick={() => update({ q: undefined, mode: undefined })}
              className="flex min-h-11 items-center gap-1 rounded-full border border-mint/30 bg-mint/10 px-3 text-caption text-mint md:min-h-7"
            >
              “{search.q}” <X className="size-3" aria-hidden />
            </button>
          </div>
        ) : null}

        {liveCapped ? (
          <div className="rounded-lg border border-gold/30 bg-gold/10 px-3 py-2 text-label text-gold">
            The live stream ended at {formatCount(live.rows.length)} of{' '}
            {formatCount(live.done!.total_matched)} matching rows — the server caps streams. Switch
            live off to page through the full set.
          </div>
        ) : null}
      </div>

      <div
        aria-live="polite"
        className="shrink-0 px-3 pt-2 pb-1.5 font-mono text-caption text-ink-faint md:px-4"
      >
        {isLoading
          ? 'loading…'
          : total !== undefined
            ? `${rows.length > 0 ? `1–${formatCount(rows.length)} of ` : ''}${totalExact ? '' : '~'}${formatCount(total)}${
                isSearchMode
                  ? ` hits · ${contentQ.data?.took_ms ?? 0} ms`
                  : ` pages · sorted by ${(search.sort ?? 'updated_desc').replace('_', ' ')}`
              }`
            : ''}
      </div>

      {currentError ? (
        <ErrorState
          error={currentError}
          title={isSearchMode ? 'Search failed' : 'The list could not load'}
          onRetry={() => (isSearchMode ? contentQ.refetch() : listQ.refetch())}
        />
      ) : isLoading ? (
        <div className="space-y-2 px-3 pt-2 md:px-4" aria-hidden>
          {Array.from({ length: 8 }, (_, i) => (
            <div key={i} className="flex items-center gap-3">
              <Skeleton className="size-4.5 rounded" />
              <div className="flex-1 space-y-1.5">
                <Skeleton className="h-4 w-3/5" />
                <Skeleton className="h-3 w-2/5" />
              </div>
            </div>
          ))}
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<SearchX aria-hidden />}
          title={isSearchMode ? `No matches for “${search.q}”` : 'No pages match'}
          description={recap !== '' ? `Filters: ${recap}` : undefined}
          action={
            <Button
              variant="secondary"
              onClick={() =>
                void navigate({ to: '/pages', search: isSearchMode ? { q: search.q } : {} })
              }
            >
              Clear filters
            </Button>
          }
        />
      ) : (
        <DocumentList
          rows={rows}
          mode={isSearchMode ? 'search' : 'list'}
          activeIndex={clampedActive}
          onActiveIndexChange={setActiveIndex}
          selected={selected}
          onToggleSelect={toggleSelect}
          onInspect={inspect}
          hasMore={!isSearchMode && !liveActive && listQ.hasNextPage}
          isFetchingMore={listQ.isFetchingNextPage}
          onLoadMore={() => void listQ.fetchNextPage()}
          sort={search.sort ?? 'updated_desc'}
          onSortChange={(sort) => update({ sort: sort === 'updated_desc' ? undefined : sort })}
          listRef={(api) => {
            listApi.current = api;
          }}
          renderExtraActions={(row) => (
            <>
              <MenuItem
                icon={row.hidden ? <Eye aria-hidden /> : <EyeOff aria-hidden />}
                onSelect={() => hidePages.mutate({ ids: [row.id], hidden: !row.hidden })}
              >
                {row.hidden ? 'Unhide' : 'Hide from search'}
              </MenuItem>
              <MenuItem
                destructive
                icon={<Trash2 aria-hidden />}
                onSelect={() => setDeleteIds([row.id])}
              >
                Delete
              </MenuItem>
            </>
          )}
        />
      )}

      {rows.length > 30 ? (
        <button
          type="button"
          onClick={() => {
            listApi.current?.scrollToIndex(0);
            setActiveIndex(0);
          }}
          className="glass-panel fixed right-4 bottom-[calc(84px+env(safe-area-inset-bottom))] z-30 flex size-11 items-center justify-center rounded-full text-ink-mute transition-colors hover:text-ink lg:bottom-16"
          aria-label="Back to top"
        >
          <ArrowUp className="size-4" aria-hidden />
        </button>
      ) : null}

      <SelectionBar
        selectedCount={selected.size}
        urls={rows.filter((r) => selected.has(r.id)).map((r) => r.link ?? r.id)}
        onClear={clearSelection}
      >
        <Button
          variant="ghost"
          size="sm"
          disabled={hidePages.isPending}
          onClick={() => hidePages.mutate({ ids: [...selected], hidden: true })}
        >
          <EyeOff className="size-4" aria-hidden />
          <span className="hidden sm:inline">Hide</span>
        </Button>
        <Button variant="destructive" size="sm" onClick={() => setDeleteIds([...selected])}>
          <Trash2 className="size-4" aria-hidden />
          <span className="hidden sm:inline">Delete</span>
        </Button>
      </SelectionBar>

      <AlertDialog
        open={deleteIds !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteIds(null);
        }}
        title={
          deleteIds && deleteIds.length === 1
            ? 'Delete this document?'
            : `Delete ${formatCount(deleteIds?.length ?? 0)} documents?`
        }
        actions={
          <Button variant="destructive" disabled={batchDelete.isPending} onClick={confirmDelete}>
            {batchDelete.isPending ? 'Deleting…' : 'Delete permanently'}
          </Button>
        }
      >
        <div className="space-y-2">
          {deleteIds && deleteIds.length === 1 ? (
            <p className="font-mono text-mono-sm break-all text-ink">{deleteIds[0]}</p>
          ) : null}
          <p>
            Documents and their index chunks are removed immediately and permanently — there is no
            undo. Pages owned by ACTIVE connectors are liable to be re-crawled on the next refresh.
          </p>
          <p>The result reports exactly what was deleted, per document.</p>
        </div>
      </AlertDialog>
    </div>
  );
}
