import { useState } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { Pause, Play } from 'lucide-react';
import { backgroundErrorsQuery, indexingAttemptsQuery, overviewQuery } from '@/api/queries';
import { cn } from '@/lib/cn';
import { count as formatCount, relative } from '@/lib/format';
import { Button } from '@/components/primitives/Button';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { Stat } from '@/components/primitives/Stat';
import { activityRoute } from '@/routes/activity';
import { AttemptRow } from './ConnectorDetailView';

const STATUS_FILTERS = ['SUCCESS', 'FAILED', 'CANCELED', 'COMPLETED_WITH_ERRORS', 'NOT_STARTED'] as const;

/**
 * What is the crawler doing right now (replaces ssh + psql).
 * 5 s auto-refresh while the tab is visible; a NOT_STARTED attempt is normal
 * queuing, `stalled` comes only from the backend's heartbeat heuristic.
 * (No "workers seen" — the API does not expose which worker ran an attempt.)
 */
export function ActivityView() {
  const search = activityRoute.useSearch();
  const navigate = useNavigate();
  const [autoRefresh, setAutoRefresh] = useState(true);

  const overview = useQuery({
    ...overviewQuery,
    refetchInterval: autoRefresh ? 5_000 : false,
  });
  const inProgress = useQuery({
    ...indexingAttemptsQuery('IN_PROGRESS', 50),
    refetchInterval: autoRefresh ? 5_000 : false,
  });
  const recent = useQuery({
    ...indexingAttemptsQuery(search.status, 50),
    refetchInterval: autoRefresh ? 15_000 : false,
  });

  const crawl = overview.data?.crawl;
  const recentItems = (recent.data?.items ?? []).filter((a) => a.status !== 'IN_PROGRESS');

  return (
    <div className="h-full overflow-y-auto overscroll-contain">
      <div className="mx-auto max-w-4xl space-y-4 p-3 pb-24 md:p-4">
        <div className="flex items-center justify-between gap-2">
          <h1 className="font-display font-display-soft text-display text-ink">Activity</h1>
          <Button variant="ghost" size="sm" onClick={() => setAutoRefresh((a) => !a)} aria-pressed={autoRefresh}>
            {autoRefresh ? <Pause className="size-4" aria-hidden /> : <Play className="size-4" aria-hidden />}
            {autoRefresh ? 'Auto-refresh on' : 'Auto-refresh paused'}
          </Button>
        </div>

        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          <Stat label="Docs · last 15 min" value={crawl ? formatCount(crawl.docs_last_15m) : '…'} />
          <Stat label="Docs · last 24 h" value={crawl ? formatCount(crawl.docs_last_24h) : '…'} />
          <Stat label="Attempts running" value={crawl ? formatCount(crawl.attempts_in_progress) : '…'} />
          <Stat
            label="Stalled"
            value={crawl ? formatCount(crawl.attempts_stalled) : '…'}
            tone={crawl && crawl.attempts_stalled > 0 ? 'gold' : 'default'}
            caption={crawl && crawl.attempts_stalled > 0 ? 'no heartbeat for 45 min' : undefined}
          />
        </div>

        <section aria-label="In progress" className="space-y-2.5">
          <h2 className="text-label font-medium text-ink-faint">In progress</h2>
          {inProgress.isPending ? (
            <Skeleton className="h-24 rounded-xl" />
          ) : inProgress.isError ? (
            <ErrorState error={inProgress.error} title="Live attempts could not load" onRetry={() => void inProgress.refetch()} />
          ) : inProgress.data.items.length === 0 ? (
            <p className="rounded-xl border border-line bg-surface px-4 py-3 text-body text-ink-mute">
              Nothing is crawling right now.
            </p>
          ) : (
            inProgress.data.items.map((a) => <AttemptRow key={a.id} attempt={a} showConnector />)
          )}
        </section>

        <BackgroundErrors autoRefresh={autoRefresh} />

        <section aria-label="Recent attempts" className="space-y-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-label font-medium text-ink-faint">Recent</h2>
            <div role="group" aria-label="Filter by status" className="flex flex-wrap items-center gap-1.5">
              {STATUS_FILTERS.map((s) => {
                const active = search.status === s;
                return (
                  <button
                    key={s}
                    type="button"
                    aria-pressed={active}
                    onClick={() =>
                      void navigate({
                        to: '/activity',
                        replace: true,
                        search: { status: active ? undefined : s },
                      })
                    }
                    className={cn(
                      'min-h-11 rounded-full border px-3 text-caption transition-colors md:min-h-7',
                      active
                        ? 'border-gold/40 bg-gold/15 text-gold'
                        : 'border-line bg-surface text-ink-mute hover:bg-hover',
                    )}
                  >
                    {s === 'COMPLETED_WITH_ERRORS' ? 'PARTIAL' : s === 'NOT_STARTED' ? 'QUEUED' : s}
                  </button>
                );
              })}
            </div>
          </div>

          {recent.isPending ? (
            <Skeleton className="h-40 rounded-xl" />
          ) : recent.isError ? (
            <ErrorState error={recent.error} title="Attempts could not load" onRetry={() => void recent.refetch()} />
          ) : recentItems.length === 0 ? (
            <EmptyState title="No attempts match" description="Try a different status filter." />
          ) : (
            recentItems.map((a) => <AttemptRow key={a.id} attempt={a} showConnector />)
          )}
        </section>
      </div>
    </div>
  );
}

/**
 * Errors the indexing workers recorded outside any single attempt.
 *
 * Onyx keeps these in their own table precisely because they do not belong to
 * an attempt — a Celery task that died, a connector that threw before it could
 * open one. Nothing in the attempt list will ever mention them, so a run can
 * look clean while these pile up underneath it. Silent by default: the section
 * only appears when there is something to say.
 */
function BackgroundErrors({ autoRefresh }: { autoRefresh: boolean }) {
  const errors = useQuery({
    ...backgroundErrorsQuery(undefined, 25),
    refetchInterval: autoRefresh ? 30_000 : false,
  });

  if (errors.isPending || errors.isError || (errors.data?.length ?? 0) === 0) return null;

  return (
    <section aria-label="Background errors" className="space-y-2.5">
      <h2 className="text-label font-medium text-ink-faint">
        Background errors
        <span className="ml-2 text-ink-mute">{formatCount(errors.data.length)}</span>
      </h2>
      <p className="text-caption text-ink-mute">
        Recorded by the indexing workers outside any one attempt, so they never appear on an
        attempt&rsquo;s own error list.
      </p>
      <ul className="divide-y divide-line rounded-xl border border-line bg-surface">
        {errors.data.map((error) => (
          <li key={error.id} className="px-4 py-2.5">
            <p className="text-body text-ink">{error.message}</p>
            <p className="mt-0.5 text-caption text-ink-faint">
              {relative(error.time_created)}
              {error.cc_pair_id !== null ? ` · cc-pair ${error.cc_pair_id}` : ''}
            </p>
          </li>
        ))}
      </ul>
    </section>
  );
}
