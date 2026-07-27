import { useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import {
  ArrowLeft,
  ChevronDown,
  ExternalLink,
  Pause,
  Pencil,
  Play,
  RefreshCcw,
  Scissors,
  Trash2,
} from 'lucide-react';
import {
  connectorAttemptsQuery,
  connectorDetailQuery,
  connectorDocsQuery,
  connectorErrorsQuery,
} from '@/api/queries';
import { usePauseResume, useConnectorPrune, useTargetedReindex } from '@/api/mutations';
import type { HistoryPoint, IndexAttemptItem } from '@/api/types';
import { cn } from '@/lib/cn';
import {
  absolute,
  compact,
  count as formatCount,
  duration,
  frequency,
  relative,
  sourceLabel,
} from '@/lib/format';
import { Badge, statusTone } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from '@/components/primitives/Tabs';
import { connectorDetailRoute, type ConnectorTab } from '@/routes/connectors/ccPairId';
import {
  DeleteConnectorDialog,
  ParkedBadge,
  RenameDialog,
  RunOnceDialog,
  StatusDot,
} from './connectorShared';

const PROXY_HOST = '192.168.4.100:8765';

/** 7-day docs-added sparkline — detail view only (the list has no history). */
function Sparkline({ points }: { points: HistoryPoint[] }) {
  if (points.length === 0) return null;
  const max = Math.max(...points.map((p) => p.docs_added), 1);
  const w = 220;
  const h = 40;
  const step = w / Math.max(points.length - 1, 1);
  const path = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${(i * step).toFixed(1)},${(h - (p.docs_added / max) * (h - 4) - 2).toFixed(1)}`)
    .join(' ');
  const total = points.reduce((sum, p) => sum + p.docs_added, 0);
  return (
    <figure aria-label={`${formatCount(total)} documents added over the last ${points.length} days`}>
      <svg viewBox={`0 0 ${w} ${h}`} className="h-10 w-full max-w-56" role="img" aria-hidden>
        <path d={path} fill="none" stroke="var(--color-mint)" strokeWidth="1.5" />
      </svg>
      <figcaption className="font-mono text-caption text-ink-faint">
        +{formatCount(total)} docs · 7 days
      </figcaption>
    </figure>
  );
}

/** "just now" | "5 minutes ago" — straight from `relative`, no munging. */
function heartbeatAge(attempt: IndexAttemptItem): string | null {
  if (!attempt.last_heartbeat_time) return null;
  return relative(attempt.last_heartbeat_time);
}

export function AttemptRow({ attempt, showConnector }: { attempt: IndexAttemptItem; showConnector?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const inProgress = attempt.status === 'IN_PROGRESS';
  const started = attempt.time_started ? Date.parse(attempt.time_started) : null;
  // For running attempts time_updated advances with each heartbeat, so
  // started→updated is the honest "elapsed so far" without a render clock.
  const elapsed =
    started !== null ? duration((Date.parse(attempt.time_updated) - started) / 1000) : null;
  const progress =
    attempt.total_batches !== null && attempt.total_batches > 0
      ? Math.min(1, attempt.completed_batches / attempt.total_batches)
      : null;

  return (
    <Card
      className={cn(
        'space-y-2 p-3.5',
        attempt.stalled && 'border-gold/40',
      )}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={statusTone(attempt.status)}>
          {attempt.status === 'NOT_STARTED' ? 'QUEUED' : attempt.status}
        </Badge>
        {attempt.parked ? <ParkedBadge /> : null}
        {attempt.stalled ? (
          <Badge tone="gold" title="No heartbeat for 45+ minutes — the same heuristic the resilience cron uses">
            stalled
            {attempt.last_heartbeat_time ? ` · last heartbeat ${heartbeatAge(attempt)}` : ''}
          </Badge>
        ) : null}
        {showConnector && attempt.connector_name ? (
          <Link
            to="/connectors/$ccPairId"
            params={{ ccPairId: attempt.cc_pair_id }}
            className="font-display text-body font-medium text-ink hover:text-gold-bright"
          >
            {attempt.connector_name}
          </Link>
        ) : null}
        <span className="ml-auto font-mono text-caption text-ink-faint">
          #{attempt.id} · {relative(attempt.time_updated)}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-caption text-ink-mute">
        <span>+{formatCount(attempt.new_docs_indexed ?? 0)} docs</span>
        <span>{formatCount(attempt.total_chunks)} chunks</span>
        {attempt.pages_per_min !== null ? <span className="text-mint">{attempt.pages_per_min.toFixed(1)} pages/min</span> : null}
        {elapsed ? <span>{elapsed} elapsed</span> : null}
        {inProgress && attempt.last_heartbeat_time && !attempt.stalled ? (
          <span className="flex items-center gap-1">
            <span aria-hidden className="size-1.5 rounded-full bg-mint animate-pulse-dot" />
            heartbeat {heartbeatAge(attempt)}
          </span>
        ) : null}
        {attempt.total_failures_batch_level > 0 ? (
          <span className="text-rose">{attempt.total_failures_batch_level} batch failures</span>
        ) : null}
      </div>

      {inProgress ? (
        <div
          role="progressbar"
          aria-label="Batch progress"
          aria-valuenow={progress !== null ? Math.round(progress * 100) : undefined}
          aria-valuetext={
            progress !== null
              ? `${attempt.completed_batches} of ${attempt.total_batches} batches`
              : `${attempt.completed_batches} batches so far`
          }
          className="h-1.5 overflow-hidden rounded-full bg-line"
        >
          <div
            className={cn('h-full rounded-full bg-mint', progress === null && 'w-1/4 animate-pulse')}
            style={progress !== null ? { width: `${Math.max(progress * 100, 2)}%` } : undefined}
          />
        </div>
      ) : null}

      {attempt.error_msg ? (
        <div>
          <button
            type="button"
            onClick={() => setExpanded((e) => !e)}
            className="flex items-center gap-1 text-caption text-ink-faint hover:text-ink-mute"
            aria-expanded={expanded}
          >
            <ChevronDown className={cn('size-3.5 transition-transform', expanded && 'rotate-180')} aria-hidden />
            message
          </button>
          {expanded ? (
            <pre className="mt-1.5 overflow-x-auto rounded-lg border border-line bg-well p-2.5 font-mono text-caption whitespace-pre-wrap text-ink-mute select-all">
              {attempt.error_msg}
            </pre>
          ) : null}
        </div>
      ) : null}
    </Card>
  );
}

function AttemptsTab({ ccPairId }: { ccPairId: number }) {
  const attempts = useInfiniteQuery(connectorAttemptsQuery(ccPairId));
  if (attempts.isPending) return <Skeleton className="h-40 rounded-xl" />;
  if (attempts.isError)
    return <ErrorState error={attempts.error} title="Attempts could not load" onRetry={() => void attempts.refetch()} />;
  const items = attempts.data.pages.flatMap((p) => p.items);
  if (items.length === 0) return <EmptyState title="No attempts yet" />;
  return (
    <div className="space-y-2.5">
      {items.map((a) => (
        <AttemptRow key={a.id} attempt={a} />
      ))}
      {attempts.hasNextPage ? (
        <div className="flex justify-center">
          <Button variant="secondary" onClick={() => void attempts.fetchNextPage()} disabled={attempts.isFetchingNextPage}>
            {attempts.isFetchingNextPage ? 'Loading…' : 'Load older attempts'}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function ErrorsTab({ ccPairId }: { ccPairId: number }) {
  const errors = useInfiniteQuery(connectorErrorsQuery(ccPairId));
  const reindex = useTargetedReindex(ccPairId);
  if (errors.isPending) return <Skeleton className="h-40 rounded-xl" />;
  if (errors.isError)
    return <ErrorState error={errors.error} title="Errors could not load" onRetry={() => void errors.refetch()} />;
  const first = errors.data.pages[0];
  const items = errors.data.pages.flatMap((p) => p.items);

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-caption text-ink-faint">
          Rolling {first?.window ?? '24h'} window — older failures have been pruned; an empty list
          is not “no failures ever”.
        </p>
        {items.length > 0 ? (
          <Button variant="secondary" size="sm" disabled={reindex.isPending} onClick={() => reindex.mutate()}>
            <RefreshCcw className="size-4" aria-hidden />
            {reindex.isPending ? 'Starting…' : 'Reindex failed docs'}
          </Button>
        ) : null}
      </div>

      {items.length === 0 ? (
        <EmptyState title="No failures in the window" description={`Nothing failed in the last ${first?.window ?? '24h'}.`} />
      ) : (
        <ul className="space-y-2">
          {items.map((e) => (
            <li key={e.id} className="rounded-lg border border-line bg-surface p-3">
              <div className="flex flex-wrap items-center gap-2">
                {e.document_link ?? e.document_id ? (
                  <span className="min-w-0 flex-1 truncate font-mono text-caption text-ink-mute">
                    {e.document_link ?? e.document_id}
                  </span>
                ) : (
                  <span className="text-caption text-ink-faint">no document id</span>
                )}
                {e.is_resolved ? <Badge tone="mint">resolved</Badge> : null}
                <span className="font-mono text-caption text-ink-faint">{relative(e.time_created)}</span>
              </div>
              <p className="mt-1 text-label break-words text-rose/90">{e.failure_message}</p>
            </li>
          ))}
        </ul>
      )}

      {errors.hasNextPage ? (
        <div className="flex justify-center">
          <Button variant="secondary" onClick={() => void errors.fetchNextPage()} disabled={errors.isFetchingNextPage}>
            {errors.isFetchingNextPage ? 'Loading…' : 'Load more'}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function DocsTab({ ccPairId }: { ccPairId: number }) {
  const docs = useInfiniteQuery(connectorDocsQuery(ccPairId));
  const navigate = useNavigate();
  if (docs.isPending) return <Skeleton className="h-40 rounded-xl" />;
  if (docs.isError)
    return <ErrorState error={docs.error} title="Documents could not load" onRetry={() => void docs.refetch()} />;
  const first = docs.data.pages[0];
  const items = docs.data.pages.flatMap((p) => p.items);

  return (
    <div className="space-y-2">
      <p className="font-mono text-caption text-ink-faint">
        {first ? `${formatCount(first.total)} documents (authoritative, includes shared docs)` : ''}
      </p>
      <ul className="divide-y divide-line/60">
        {items.map((d) => (
          <li key={d.id}>
            <button
              type="button"
              onClick={() => void navigate({ to: '/pages/$docId', params: { docId: d.id } })}
              className="flex w-full flex-col items-start gap-0.5 px-1 py-2 text-left transition-colors hover:bg-hover/60"
            >
              <span className="w-full truncate text-body text-ink">{d.semantic_id}</span>
              <span className="w-full truncate font-mono text-caption text-ink-faint">{d.link ?? d.id}</span>
            </button>
          </li>
        ))}
      </ul>
      {docs.hasNextPage ? (
        <div className="flex justify-center pt-1">
          <Button variant="secondary" onClick={() => void docs.fetchNextPage()} disabled={docs.isFetchingNextPage}>
            {docs.isFetchingNextPage ? 'Loading…' : 'Load more'}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

export function ConnectorDetailView() {
  const { ccPairId } = useParams({ from: '/connectors/$ccPairId' });
  const search = connectorDetailRoute.useSearch();
  const navigate = useNavigate();
  const detail = useQuery(connectorDetailQuery(ccPairId));
  const pauseResume = usePauseResume();
  const prune = useConnectorPrune(ccPairId);
  const [dialog, setDialog] = useState<'run' | 'rename' | 'delete' | null>(null);

  const config = useMemo(() => detail.data?.connector_specific_config ?? null, [detail.data]);
  const baseUrl = config && typeof config.base_url === 'string' ? config.base_url : null;
  const viaProxy = baseUrl?.includes(PROXY_HOST) ?? false;

  if (detail.isPending) {
    return (
      <div className="space-y-3 p-4" aria-hidden>
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-32 rounded-xl" />
        <Skeleton className="h-64 rounded-xl" />
      </div>
    );
  }
  if (detail.isError) {
    return (
      <ErrorState error={detail.error} title="Connector could not load" onRetry={() => void detail.refetch()} />
    );
  }

  const c = detail.data;
  const paused = c.status === 'PAUSED';
  const tab: ConnectorTab = search.tab ?? 'attempts';

  return (
    <div className="h-full overflow-y-auto overscroll-contain">
      <div className="mx-auto max-w-4xl space-y-4 p-3 pb-24 md:p-4">
        <div className="flex items-center gap-2">
          <Link
            to="/connectors"
            className="flex size-11 items-center justify-center rounded-lg text-ink-mute transition-colors hover:bg-hover hover:text-ink md:size-8"
            aria-label="Back to connectors"
          >
            <ArrowLeft className="size-4" aria-hidden />
          </Link>
          <StatusDot status={c.status} className="size-2.5" />
          <h1 className="min-w-0 truncate font-display font-display-soft text-display text-ink">
            {c.name}
          </h1>
          <Badge tone={statusTone(c.status)}>{c.status}</Badge>
          {c.parked ? <ParkedBadge /> : null}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={pauseResume.isPending}
            onClick={() => pauseResume.mutate({ ids: [ccPairId], action: paused ? 'resume' : 'pause' })}
          >
            {paused ? <Play className="size-4" aria-hidden /> : <Pause className="size-4" aria-hidden />}
            {paused ? 'Resume' : 'Pause'}
          </Button>
          <Button variant="secondary" size="sm" onClick={() => setDialog('run')}>
            <Play className="size-4" aria-hidden /> Run now
          </Button>
          <Button variant="secondary" size="sm" disabled={prune.isPending} onClick={() => prune.mutate()}>
            <Scissors className="size-4" aria-hidden /> Prune
          </Button>
          <Button variant="secondary" size="sm" onClick={() => setDialog('rename')}>
            <Pencil className="size-4" aria-hidden /> Rename
          </Button>
          <Button variant="destructive" size="sm" onClick={() => setDialog('delete')}>
            <Trash2 className="size-4" aria-hidden /> Delete
          </Button>
        </div>

        <div className="grid gap-3 md:grid-cols-2">
          <Card className="space-y-2.5">
            <h2 className="text-label font-medium text-ink-faint">Configuration</h2>
            {baseUrl ? (
              <div className="flex items-center gap-2">
                <a
                  href={baseUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="min-w-0 flex-1 truncate font-mono text-mono-sm text-teal hover:underline"
                >
                  {baseUrl}
                  <ExternalLink className="ml-1 inline size-3" aria-hidden />
                </a>
                {viaProxy ? (
                  <Badge tone="teal" title="Routed through the CF-bypass connector proxy on infra">
                    via proxy
                  </Badge>
                ) : null}
              </div>
            ) : null}
            <dl className="grid grid-cols-2 gap-x-4 gap-y-1.5 font-mono text-caption text-ink-mute">
              <dt className="text-ink-faint">source</dt>
              <dd>{sourceLabel(c.source)}</dd>
              <dt className="text-ink-faint">type</dt>
              <dd>{typeof config?.web_connector_type === 'string' ? config.web_connector_type : (c.input_type ?? '—')}</dd>
              <dt className="text-ink-faint">refresh</dt>
              <dd>{c.refresh_freq_secs !== null ? frequency(c.refresh_freq_secs) : 'not scheduled'}</dd>
              <dt className="text-ink-faint">prune</dt>
              <dd>{c.prune_freq_secs !== null ? frequency(c.prune_freq_secs) : '—'}</dd>
              <dt className="text-ink-faint">credential</dt>
              <dd>{c.credential_name ?? (c.credential_id !== null ? `#${c.credential_id}` : '—')}</dd>
              <dt className="text-ink-faint">created</dt>
              <dd>{c.time_created ? absolute(c.time_created) : '—'}</dd>
              <dt className="text-ink-faint">last pruned</dt>
              <dd>{c.last_pruned ? relative(c.last_pruned) : 'never'}</dd>
              <dt className="text-ink-faint">indexing trigger</dt>
              <dd>{c.indexing_trigger ?? '—'}</dd>
            </dl>
          </Card>

          <Card className="space-y-3">
            <h2 className="text-label font-medium text-ink-faint">Documents & activity</h2>
            <div className="flex items-baseline gap-3">
              <span className="stat-numeral text-display-xl text-ink">{compact(c.doc_count)}</span>
              <span className="font-mono text-caption text-ink-faint">documents</span>
            </div>
            {c.history && c.history.length > 0 ? <Sparkline points={c.history} /> : null}
            <dl className="grid grid-cols-2 gap-x-4 gap-y-1.5 font-mono text-caption text-ink-mute">
              <dt className="text-ink-faint">last success</dt>
              <dd>{c.last_successful_index_time ? relative(c.last_successful_index_time) : 'never'}</dd>
              <dt className="text-ink-faint">attempts</dt>
              <dd>
                <span className="text-mint">{c.attempts.success} ok</span>
                {' · '}
                <span className="text-rose">{c.attempts.failed} failed</span>
                {' · '}
                {c.attempts.canceled} canceled
              </dd>
            </dl>
          </Card>
        </div>

        <TabsRoot
          value={tab}
          onValueChange={(v) =>
            void navigate({
              to: '/connectors/$ccPairId',
              params: { ccPairId },
              replace: true,
              search: { tab: v === 'attempts' ? undefined : (v as ConnectorTab) },
            })
          }
        >
          <TabsList>
            <TabsTrigger value="attempts">Attempts</TabsTrigger>
            <TabsTrigger value="errors">Errors</TabsTrigger>
            <TabsTrigger value="documents">Documents</TabsTrigger>
          </TabsList>
          <TabsContent value="attempts" className="pt-3">
            <AttemptsTab ccPairId={ccPairId} />
          </TabsContent>
          <TabsContent value="errors" className="pt-3">
            <ErrorsTab ccPairId={ccPairId} />
          </TabsContent>
          <TabsContent value="documents" className="pt-3">
            <DocsTab ccPairId={ccPairId} />
          </TabsContent>
        </TabsRoot>
      </div>

      {dialog === 'run' ? <RunOnceDialog connector={c} open onOpenChange={(o) => !o && setDialog(null)} /> : null}
      {dialog === 'rename' ? <RenameDialog connector={c} open onOpenChange={(o) => !o && setDialog(null)} /> : null}
      {dialog === 'delete' ? <DeleteConnectorDialog connector={c} open onOpenChange={(o) => !o && setDialog(null)} /> : null}
    </div>
  );
}
