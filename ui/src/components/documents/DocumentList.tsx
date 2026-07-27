import { useEffect, useRef, type ReactNode } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  ArrowDown,
  ArrowUp,
  Copy,
  ExternalLink,
  EyeOff,
  FileSearch,
  MoreHorizontal,
} from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/cn';
import { absolute, compact, relative } from '@/lib/format';
import { Checkbox } from '@/components/primitives/Checkbox';
import { IconButton } from '@/components/primitives/Button';
import {
  MenuRoot,
  MenuTrigger,
  MenuContent,
  MenuItem,
  MenuSeparator,
} from '@/components/primitives/Menu';
import { useContainerWidth } from '@/hooks/useContainerWidth';
import type { PagesSort } from '@/routes/pages';

/** Unified row for both the list endpoint and content-search hits. */
export interface ExplorerRow {
  id: string;
  title: string;
  link: string | null;
  connectorId: number | null;
  connectorName: string | null;
  source: string | null;
  /** null = "Onyx has not counted this yet" — rendered as —, never as 0. */
  chunkCount: number | null;
  updatedAt: string | null;
  hidden: boolean;
  boost: number;
  score?: number;
  snippet?: string | null;
}

/** Render an `<em>`-highlighted snippet without dangerouslySetInnerHTML. */
export function SnippetText({ snippet }: { snippet: string }) {
  const parts = snippet.split(/<\/?em>/);
  return (
    <span className="line-clamp-2 text-label text-ink-mute">
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <em key={i} className="font-display font-medium text-mint not-italic md:italic">
            {part}
          </em>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </span>
  );
}

function RowActions({ row, onInspect, extraItems }: { row: ExplorerRow; onInspect: () => void; extraItems?: ReactNode }) {
  return (
    <MenuRoot>
      <MenuTrigger asChild>
        <IconButton
          label="Row actions"
          className="hover-capable:opacity-0 hover-capable:group-hover:opacity-100 data-[state=open]:opacity-100 focus-visible:opacity-100"
          onClick={(e) => e.stopPropagation()}
        >
          <MoreHorizontal className="size-4" aria-hidden />
        </IconButton>
      </MenuTrigger>
      <MenuContent onClick={(e) => e.stopPropagation()}>
        <MenuItem icon={<FileSearch aria-hidden />} onSelect={onInspect}>
          Inspect
        </MenuItem>
        {row.link ? (
          <MenuItem
            icon={<ExternalLink aria-hidden />}
            onSelect={() => window.open(row.link!, '_blank', 'noopener')}
          >
            Open link
          </MenuItem>
        ) : null}
        <MenuItem
          icon={<Copy aria-hidden />}
          onSelect={() => {
            void navigator.clipboard.writeText(row.link ?? row.id);
            toast('URL copied');
          }}
        >
          Copy URL
        </MenuItem>
        {extraItems ? (
          <>
            <MenuSeparator />
            {extraItems}
          </>
        ) : null}
      </MenuContent>
    </MenuRoot>
  );
}

function ScoreBar({ score, max }: { score: number; max: number }) {
  const pct = max > 0 ? Math.max(4, Math.round((score / max) * 100)) : 0;
  return (
    <div aria-hidden className="h-0.5 w-24 overflow-hidden rounded-full bg-line">
      <div
        className="h-full rounded-full bg-linear-to-r from-gold/60 to-gold"
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

interface DocumentListProps {
  rows: ExplorerRow[];
  mode: 'list' | 'search';
  activeIndex: number;
  onActiveIndexChange: (index: number) => void;
  selected: Set<string>;
  onToggleSelect: (id: string, range: boolean) => void;
  onInspect: (row: ExplorerRow) => void;
  hasMore?: boolean;
  isFetchingMore?: boolean;
  onLoadMore?: () => void;
  sort?: PagesSort;
  onSortChange?: (sort: PagesSort) => void;
  /** Per-row extra menu entries (Hide/Delete arrive with M3). */
  renderExtraActions?: (row: ExplorerRow) => ReactNode;
  /** Imperative scroll access for j/k keyboard navigation. */
  listRef?: (api: { scrollToIndex: (i: number) => void }) => void;
}

const CARD_BREAK = 640; // 40rem — below this the container renders cards
const WIDE_BREAK = 832; // 52rem — connector/updated columns drop below this

export function DocumentList({
  rows,
  mode,
  activeIndex,
  onActiveIndexChange,
  selected,
  onToggleSelect,
  onInspect,
  hasMore,
  isFetchingMore,
  onLoadMore,
  sort = 'updated_desc',
  onSortChange,
  renderExtraActions,
  listRef,
}: DocumentListProps) {
  const [containerRef, width] = useContainerWidth<HTMLDivElement>();
  const scrollRef = useRef<HTMLDivElement>(null);
  const isCards = width > 0 && width < CARD_BREAK;
  const showWide = width >= WIDE_BREAK;

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => (isCards ? (mode === 'search' ? 148 : 108) : mode === 'search' ? 84 : 56),
    overscan: 12,
    getItemKey: (i) => rows[i]?.id ?? i,
  });

  useEffect(() => {
    listRef?.({
      scrollToIndex: (i) => virtualizer.scrollToIndex(i, { align: 'auto' }),
    });
  }, [listRef, virtualizer]);

  // Shape change (cards <-> table) invalidates every measured height.
  useEffect(() => {
    virtualizer.measure();
  }, [isCards, mode, virtualizer]);

  const virtualItems = virtualizer.getVirtualItems();
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? 0;

  useEffect(() => {
    if (hasMore && !isFetchingMore && rows.length > 0 && lastIndex >= rows.length - 30) {
      onLoadMore?.();
    }
  }, [lastIndex, rows.length, hasMore, isFetchingMore, onLoadMore]);

  const maxScore = mode === 'search' ? Math.max(...rows.map((r) => r.score ?? 0), 0) : 0;

  const sortHeader = (label: string, asc: PagesSort, desc: PagesSort, alignRight?: boolean) => {
    const active = sort === asc || sort === desc;
    const nextSort: PagesSort = sort === desc ? asc : desc;
    return (
      <button
        type="button"
        onClick={() => onSortChange?.(nextSort)}
        className={cn(
          'flex items-center gap-1 text-label transition-colors hover:text-ink',
          active ? 'text-gold' : 'text-ink-mute',
          alignRight && 'justify-end',
        )}
      >
        {label}
        {active ? (
          sort === desc ? (
            <ArrowDown className="size-3.5" aria-hidden />
          ) : (
            <ArrowUp className="size-3.5" aria-hidden />
          )
        ) : null}
      </button>
    );
  };

  const gridCols = showWide
    ? 'minmax(0,1fr) 9rem 4.5rem 6.5rem 2.75rem'
    : 'minmax(0,1fr) 2.75rem';

  return (
    <div ref={containerRef} className="flex min-h-0 flex-1 flex-col">
      {!isCards && rows.length > 0 ? (
        <div
          className="grid shrink-0 items-center gap-3 border-b border-line px-3 py-2 md:px-4"
          style={{ gridTemplateColumns: `1.75rem ${gridCols}` }}
        >
          <span />
          <span className="text-label text-ink-mute">Title</span>
          {showWide ? (
            <>
              <span className="text-label text-ink-mute">Connector</span>
              {sortHeader('Chunks', 'chunks_asc', 'chunks_desc', true)}
              {sortHeader('Updated', 'updated_asc', 'updated_desc')}
            </>
          ) : null}
          <span />
        </div>
      ) : null}

      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        <div
          role="list"
          aria-label="Documents"
          className="relative w-full"
          style={{ height: virtualizer.getTotalSize() }}
        >
          {virtualItems.map((vi) => {
            const row = rows[vi.index];
            if (!row) return null;
            const isSelected = selected.has(row.id);
            const isActive = vi.index === activeIndex;

            const openRow = () => {
              onActiveIndexChange(vi.index);
              onInspect(row);
            };

            const rowShell = (children: ReactNode, extraClass?: string) => (
              <div
                key={vi.key}
                data-index={vi.index}
                ref={virtualizer.measureElement}
                role="listitem"
                tabIndex={0}
                onClick={openRow}
                onKeyDown={(e) => {
                  if (e.target !== e.currentTarget) return;
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    openRow();
                  }
                }}
                className={cn(
                  'group absolute inset-x-0 top-0 cursor-pointer outline-none',
                  extraClass,
                )}
                style={{ transform: `translateY(${vi.start}px)` }}
              >
                {children}
              </div>
            );

            if (isCards) {
              return rowShell(
                <div
                  className={cn(
                    'mx-3 my-1.5 rounded-xl border p-3.5 transition-colors',
                    isSelected
                      ? 'border-gold/40 bg-active/50'
                      : isActive
                        ? 'border-line-2 bg-hover'
                        : 'border-line bg-surface',
                  )}
                >
                  <div className="flex items-start gap-3">
                    <div className="min-w-0 flex-1">
                      <h3 className="line-clamp-2 font-display text-body font-medium text-ink">
                        {row.title}
                      </h3>
                      <p className="mt-0.5 truncate font-mono text-caption text-ink-faint">
                        {row.link ?? row.id}
                      </p>
                    </div>
                    <Checkbox
                      checked={isSelected}
                      onCheckedChange={() => onToggleSelect(row.id, false)}
                      label={`Select ${row.title}`}
                      stopPropagation
                      className="mt-0.5"
                    />
                  </div>
                  {mode === 'search' && row.snippet ? (
                    <div className="mt-2">
                      <SnippetText snippet={row.snippet} />
                    </div>
                  ) : null}
                  <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-caption text-ink-faint">
                    {mode === 'search' && row.score !== undefined ? (
                      <ScoreBar score={row.score} max={maxScore} />
                    ) : null}
                    <span title={row.chunkCount === null ? 'Onyx has not counted chunks yet' : undefined}>
                      {row.chunkCount === null ? '— chunks' : `${compact(row.chunkCount)} chunks`}
                    </span>
                    {row.connectorName ? <span className="text-violet">{row.connectorName}</span> : null}
                    {row.updatedAt ? (
                      <span title={absolute(row.updatedAt)}>{relative(row.updatedAt)}</span>
                    ) : null}
                    {row.hidden ? (
                      <span className="flex items-center gap-1 text-ink-mute">
                        <EyeOff className="size-3" aria-hidden /> hidden
                      </span>
                    ) : null}
                    {row.boost !== 0 ? <span className="text-gold">boost {row.boost > 0 ? '+' : ''}{row.boost}</span> : null}
                  </div>
                </div>,
              );
            }

            return rowShell(
              <div
                className={cn(
                  'relative grid h-full items-center gap-3 border-b border-line/60 px-3 md:px-4 transition-colors',
                  isSelected ? 'bg-active/50' : 'hover:bg-hover/60',
                  isActive && 'bg-hover/40',
                )}
                style={{ gridTemplateColumns: `1.75rem ${gridCols}` }}
              >
                {isSelected || isActive ? (
                  <span aria-hidden className={cn('absolute inset-y-1.5 left-0 w-0.5 rounded-full', isSelected ? 'bg-gold' : 'bg-line-3')} />
                ) : null}
                <Checkbox
                  checked={isSelected}
                  onCheckedChange={() => onToggleSelect(row.id, false)}
                  label={`Select ${row.title}`}
                  stopPropagation
                />
                <div className="min-w-0 py-1.5">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-body text-ink">{row.title}</span>
                    {row.hidden ? (
                      <EyeOff className="size-3.5 shrink-0 text-ink-faint" aria-label="Hidden from search" />
                    ) : null}
                    {row.boost !== 0 ? (
                      <span className="shrink-0 font-mono text-caption text-gold">
                        {row.boost > 0 ? '+' : ''}
                        {row.boost}
                      </span>
                    ) : null}
                  </div>
                  <div className="flex items-center gap-2">
                    {mode === 'search' && row.score !== undefined ? (
                      <ScoreBar score={row.score} max={maxScore} />
                    ) : null}
                    <span className="truncate font-mono text-caption text-ink-faint">
                      {row.link ?? row.id}
                    </span>
                  </div>
                  {mode === 'search' && row.snippet ? (
                    <div className="mt-1 pr-4">
                      <SnippetText snippet={row.snippet} />
                    </div>
                  ) : null}
                </div>
                {showWide ? (
                  <>
                    <span className="truncate text-label text-ink-mute">{row.connectorName ?? '—'}</span>
                    <span
                      className="text-right font-mono text-mono-sm text-ink-mute"
                      title={row.chunkCount === null ? 'Onyx has not counted chunks yet' : undefined}
                    >
                      {row.chunkCount === null ? '—' : compact(row.chunkCount)}
                    </span>
                    <span
                      className="truncate text-label text-ink-mute"
                      title={row.updatedAt ? absolute(row.updatedAt) : undefined}
                    >
                      {row.updatedAt ? relative(row.updatedAt) : '—'}
                    </span>
                  </>
                ) : null}
                <RowActions
                  row={row}
                  onInspect={() => onInspect(row)}
                  extraItems={renderExtraActions?.(row)}
                />
              </div>,
            );
          })}
        </div>
        {isFetchingMore ? (
          <div className="flex justify-center py-4">
            <span className="font-mono text-caption text-ink-faint">loading more…</span>
          </div>
        ) : null}
      </div>
    </div>
  );
}
