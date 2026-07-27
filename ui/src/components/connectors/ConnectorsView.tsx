import { useMemo, useState } from 'react';
import { Link, useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { MoreHorizontal, Pause, Play, SearchX, X } from 'lucide-react';
import { connectorsQuery } from '@/api/queries';
import { usePauseResume } from '@/api/mutations';
import type { ConnectorSummary } from '@/api/types';
import { cn } from '@/lib/cn';
import { compact, count as formatCount, relative, sourceLabel } from '@/lib/format';
import { Badge, statusTone } from '@/components/primitives/Badge';
import { Button, IconButton } from '@/components/primitives/Button';
import { Checkbox } from '@/components/primitives/Checkbox';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { MenuRoot, MenuTrigger, MenuContent } from '@/components/primitives/Menu';
import { Skeleton } from '@/components/primitives/Skeleton';
import { useContainerWidth } from '@/hooks/useContainerWidth';
import { connectorsRoute, type ConnectorsSearch, type ConnectorsSort } from '@/routes/connectors';
import {
  ConnectorMenuItems,
  DeleteConnectorDialog,
  ParkedBadge,
  RenameDialog,
  RunOnceDialog,
  StatusDot,
  type ConnectorDialogKind,
} from './connectorShared';

/** Status-filter values: real statuses plus the flag pseudo-statuses `errored`/`parked`. */
type StatusFilter = 'active' | 'paused' | 'initial_indexing' | 'errored' | 'parked';

function applyFilters(list: ConnectorSummary[], search: ConnectorsSearch): ConnectorSummary[] {
  let out = list;
  if (search.status === 'errored') out = out.filter((c) => c.in_repeated_error_state);
  else if (search.status === 'parked') out = out.filter((c) => c.parked);
  else if (search.status) {
    const wanted = search.status.toUpperCase();
    out = out.filter((c) => c.status === wanted);
  }
  if (search.source) {
    const wanted = search.source.toUpperCase();
    out = out.filter((c) => c.source === wanted);
  }
  if (search.filter) {
    const needle = search.filter.toLowerCase();
    out = out.filter((c) => c.name.toLowerCase().includes(needle));
  }
  return out;
}

function sortConnectors(list: ConnectorSummary[], sort: ConnectorsSort): ConnectorSummary[] {
  const out = [...list];
  switch (sort) {
    case 'name':
      return out.sort((a, b) => a.name.localeCompare(b.name));
    case 'recent':
      return out.sort(
        (a, b) =>
          Date.parse(b.last_attempt?.time_updated ?? '1970') -
          Date.parse(a.last_attempt?.time_updated ?? '1970'),
      );
    case 'errors':
      return out.sort(
        (a, b) =>
          Number(b.in_repeated_error_state) - Number(a.in_repeated_error_state) ||
          Number(b.parked) - Number(a.parked) ||
          b.doc_count - a.doc_count,
      );
    case 'docs':
    default:
      return out.sort((a, b) => b.doc_count - a.doc_count);
  }
}

function SummaryTile({
  label,
  value,
  tone,
  active,
  onClick,
}: {
  label: string;
  value: number;
  tone?: 'mint' | 'gold' | 'rose';
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        'flex min-w-24 flex-1 snap-start flex-col items-start gap-0.5 rounded-xl border p-3 text-left transition-colors',
        active ? 'border-gold/40 bg-gold/10' : 'border-line bg-surface hover:bg-hover',
      )}
    >
      <span
        className={cn(
          'stat-numeral text-display leading-none',
          tone === 'mint' ? 'text-mint' : tone === 'gold' ? 'text-gold' : tone === 'rose' ? 'text-rose' : 'text-ink',
        )}
      >
        {value}
      </span>
      <span className="text-caption text-ink-mute">{label}</span>
    </button>
  );
}

export function ConnectorsView() {
  const search = connectorsRoute.useSearch();
  const navigate = useNavigate();
  const connectors = useQuery(connectorsQuery);
  const pauseResume = usePauseResume();
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [dialog, setDialog] = useState<{ kind: ConnectorDialogKind; connector: ConnectorSummary } | null>(null);
  const [containerRef, width] = useContainerWidth<HTMLDivElement>();
  const isCards = width > 0 && width < 720;

  const update = (patch: Partial<ConnectorsSearch>) =>
    void navigate({
      to: '/connectors',
      search: (prev) => ({ ...(prev as ConnectorsSearch), ...patch }),
    });

  const all = useMemo(() => connectors.data ?? [], [connectors.data]);
  const rows = useMemo(
    () => sortConnectors(applyFilters(all, search), search.sort ?? 'docs'),
    [all, search],
  );

  const counts = useMemo(
    () => ({
      active: all.filter((c) => c.status === 'ACTIVE').length,
      paused: all.filter((c) => c.status === 'PAUSED').length,
      initial: all.filter((c) => c.status === 'INITIAL_INDEXING').length,
      errored: all.filter((c) => c.in_repeated_error_state).length,
      parked: all.filter((c) => c.parked).length,
    }),
    [all],
  );

  const sources = useMemo(() => [...new Set(all.map((c) => c.source))].sort(), [all]);

  const toggleStatus = (value: StatusFilter) =>
    update({ status: search.status === value ? undefined : value });

  const toggleSelect = (id: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  if (connectors.isError) {
    return (
      <ErrorState
        error={connectors.error}
        title="The fleet could not load"
        onRetry={() => void connectors.refetch()}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col" ref={containerRef}>
      <div className="shrink-0 space-y-3 px-3 pt-3 md:px-4">
        <div className="flex snap-x gap-2 overflow-x-auto pb-0.5 [scrollbar-width:none]">
          <SummaryTile label="Active" value={counts.active} tone="mint" active={search.status === 'active'} onClick={() => toggleStatus('active')} />
          <SummaryTile label="Paused" value={counts.paused} active={search.status === 'paused'} onClick={() => toggleStatus('paused')} />
          <SummaryTile label="Initial indexing" value={counts.initial} tone="gold" active={search.status === 'initial_indexing'} onClick={() => toggleStatus('initial_indexing')} />
          <SummaryTile label="Repeated errors" value={counts.errored} tone="rose" active={search.status === 'errored'} onClick={() => toggleStatus('errored')} />
          <SummaryTile label="Parked" value={counts.parked} tone="gold" active={search.status === 'parked'} onClick={() => toggleStatus('parked')} />
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Input
            value={search.filter ?? ''}
            onChange={(e) => update({ filter: e.target.value || undefined })}
            placeholder="Filter by name…"
            aria-label="Filter connectors by name"
            className="max-w-56"
          />
          <select
            value={search.source ?? ''}
            onChange={(e) => update({ source: e.target.value || undefined })}
            aria-label="Filter by source"
            className="min-h-11 rounded-lg border border-line bg-well px-3 text-base text-ink md:min-h-9 md:text-body focus:border-gold/60 focus:outline-none"
          >
            <option value="">All sources</option>
            {sources.map((s) => (
              <option key={s} value={s.toLowerCase()}>
                {sourceLabel(s)}
              </option>
            ))}
          </select>
          <select
            value={search.sort ?? 'docs'}
            onChange={(e) => update({ sort: e.target.value === 'docs' ? undefined : (e.target.value as ConnectorsSort) })}
            aria-label="Sort connectors"
            className="min-h-11 rounded-lg border border-line bg-well px-3 text-base text-ink md:min-h-9 md:text-body focus:border-gold/60 focus:outline-none"
          >
            <option value="docs">Most documents</option>
            <option value="recent">Recent activity</option>
            <option value="errors">Errors first</option>
            <option value="name">Name</option>
          </select>
          <span aria-live="polite" className="ml-auto font-mono text-caption text-ink-faint">
            {connectors.isPending ? 'loading…' : `${rows.length} of ${all.length}`}
          </span>
        </div>
      </div>

      {connectors.isPending ? (
        <div className="space-y-2 px-3 pt-3 md:px-4" aria-hidden>
          {Array.from({ length: 8 }, (_, i) => (
            <Skeleton key={i} className="h-12 rounded-xl" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<SearchX aria-hidden />}
          title="No connectors match"
          action={
            <Button variant="secondary" onClick={() => update({ status: undefined, source: undefined, filter: undefined })}>
              Clear filters
            </Button>
          }
        />
      ) : (
        <div className="mt-2 min-h-0 flex-1 overflow-y-auto overscroll-contain px-3 pb-24 md:px-4">
          <ul className={cn(isCards ? 'space-y-2' : 'divide-y divide-line/60')}>
            {rows.map((c) => {
              const isSelected = selected.has(c.cc_pair_id);
              const open = () =>
                void navigate({ to: '/connectors/$ccPairId', params: { ccPairId: c.cc_pair_id } });

              const menu = (
                <MenuRoot>
                  <MenuTrigger asChild>
                    <IconButton
                      label={`Actions for ${c.name}`}
                      onClick={(e) => e.stopPropagation()}
                      className={cn(!isCards && 'hover-capable:opacity-0 hover-capable:group-hover:opacity-100 data-[state=open]:opacity-100')}
                    >
                      <MoreHorizontal className="size-4" aria-hidden />
                    </IconButton>
                  </MenuTrigger>
                  <MenuContent onClick={(e) => e.stopPropagation()}>
                    <ConnectorMenuItems
                      connector={c}
                      onDialog={(kind) => setDialog({ kind, connector: c })}
                    />
                  </MenuContent>
                </MenuRoot>
              );

              if (isCards) {
                return (
                  <li key={c.cc_pair_id}>
                    <div
                      onClick={open}
                      className={cn(
                        'cursor-pointer rounded-xl border p-3.5 transition-colors',
                        isSelected ? 'border-gold/40 bg-active/50' : 'border-line bg-surface hover:bg-hover',
                      )}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex min-w-0 items-center gap-2">
                          <StatusDot status={c.status} />
                          <Link
                            to="/connectors/$ccPairId"
                            params={{ ccPairId: c.cc_pair_id }}
                            onClick={(e) => e.stopPropagation()}
                            className="truncate font-display text-body font-medium text-ink"
                          >
                            {c.name}
                          </Link>
                          {c.parked ? <ParkedBadge /> : null}
                        </div>
                        {menu}
                      </div>
                      <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-caption text-ink-faint">
                        <Badge tone={statusTone(c.status)}>{c.status}</Badge>
                        <span>{sourceLabel(c.source)}</span>
                        <span>{compact(c.doc_count)} docs</span>
                        {c.last_successful_index_time ? (
                          <span>ok {relative(c.last_successful_index_time)}</span>
                        ) : (
                          <span className="text-ink-faint">no success yet</span>
                        )}
                        {c.in_repeated_error_state ? <span className="text-rose">repeated errors</span> : null}
                      </div>
                    </div>
                  </li>
                );
              }

              return (
                <li key={c.cc_pair_id}>
                  <div
                    onClick={open}
                    className={cn(
                      'group grid cursor-pointer items-center gap-3 px-1 py-2 transition-colors hover:bg-hover/60',
                      isSelected && 'bg-active/40',
                    )}
                    style={{ gridTemplateColumns: '1.75rem minmax(0,1.4fr) 6rem 5.5rem minmax(0,1fr) 2.75rem' }}
                  >
                    <Checkbox
                      checked={isSelected}
                      onCheckedChange={() => toggleSelect(c.cc_pair_id)}
                      label={`Select ${c.name}`}
                      stopPropagation
                    />
                    <div className="flex min-w-0 items-center gap-2">
                      <StatusDot status={c.status} />
                      <Link
                        to="/connectors/$ccPairId"
                        params={{ ccPairId: c.cc_pair_id }}
                        onClick={(e) => e.stopPropagation()}
                        className="truncate font-display text-body font-medium text-ink"
                      >
                        {c.name}
                      </Link>
                      {c.parked ? <ParkedBadge /> : null}
                      {c.in_repeated_error_state ? (
                        <span className="size-1.5 shrink-0 rounded-full bg-rose" title="In repeated error state" />
                      ) : null}
                      <span className="text-caption text-ink-faint">{sourceLabel(c.source)}</span>
                    </div>
                    <span className="text-right font-mono text-mono-sm text-ink-mute">
                      {formatCount(c.doc_count)}
                    </span>
                    <Badge tone={statusTone(c.status)} className="justify-self-start">
                      {c.status === 'INITIAL_INDEXING' ? 'INITIAL' : c.status}
                    </Badge>
                    <span className="truncate text-label text-ink-faint">
                      {c.last_attempt?.status ? (
                        <>
                          last attempt {c.last_attempt.status.toLowerCase()}
                          {c.last_attempt.time_updated ? ` · ${relative(c.last_attempt.time_updated)}` : ''}
                        </>
                      ) : (
                        'no attempts'
                      )}
                    </span>
                    {menu}
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {selected.size > 0 ? (
        <div className="pointer-events-none fixed inset-x-0 bottom-[calc(72px+env(safe-area-inset-bottom))] z-40 flex justify-center px-4 lg:bottom-6">
          <div className="glass-panel pointer-events-auto flex items-center gap-1.5 rounded-full py-1.5 pr-1.5 pl-4 animate-slide-up">
            <span className="mr-1 text-label whitespace-nowrap text-ink">{selected.size} selected</span>
            <Button
              variant="ghost"
              size="sm"
              disabled={pauseResume.isPending}
              onClick={() => pauseResume.mutate({ ids: [...selected], action: 'pause' }, { onSuccess: () => setSelected(new Set()) })}
            >
              <Pause className="size-4" aria-hidden /> Pause
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={pauseResume.isPending}
              onClick={() => pauseResume.mutate({ ids: [...selected], action: 'resume' }, { onSuccess: () => setSelected(new Set()) })}
            >
              <Play className="size-4" aria-hidden /> Resume
            </Button>
            <IconButton label="Clear selection" onClick={() => setSelected(new Set())}>
              <X className="size-4" aria-hidden />
            </IconButton>
          </div>
        </div>
      ) : null}

      {dialog?.kind === 'run' ? (
        <RunOnceDialog connector={dialog.connector} open onOpenChange={(o) => !o && setDialog(null)} />
      ) : null}
      {dialog?.kind === 'rename' ? (
        <RenameDialog connector={dialog.connector} open onOpenChange={(o) => !o && setDialog(null)} />
      ) : null}
      {dialog?.kind === 'delete' ? (
        <DeleteConnectorDialog connector={dialog.connector} open onOpenChange={(o) => !o && setDialog(null)} />
      ) : null}
    </div>
  );
}
