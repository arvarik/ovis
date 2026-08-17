/**
 * /prune — Review · Staged · Rules · History under an always-visible status
 * strip. Numbers come from /prune/status (server truths, polled), never
 * client-side arithmetic.
 */
import { useQuery } from '@tanstack/react-query';
import { getRouteApi } from '@tanstack/react-router';
import { pruneStatusQuery } from '@/api/queries';
import type { PruneStatusResponse } from '@/api/types';
import { Stat } from '@/components/primitives/Stat';
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from '@/components/primitives/Tabs';
import { count as formatCount } from '@/lib/format';
import { ClustersTab } from './ClustersTab';
import { HistoryTab } from './HistoryTab';
import { graceCountdown, useNow } from './pruneShared';
import { ReviewTab } from './ReviewTab';
import { RulesTab } from './RulesTab';
import { StagedTab } from './StagedTab';
import { TrashTab } from './TrashTab';
import { TriageTab } from './TriageTab';

const route = getRouteApi('/prune');

export type PruneTab =
  | 'triage'
  | 'review'
  | 'clusters'
  | 'staged'
  | 'trash'
  | 'rules'
  | 'history';

export function PruneView() {
  const { tab } = route.useSearch();
  const navigate = route.useNavigate();
  const status = useQuery(pruneStatusQuery);

  return (
    <div className="h-full overflow-y-auto overscroll-contain">
      <div className="mx-auto w-full max-w-6xl space-y-4 p-4 pb-24 md:p-6 md:pb-24">
        <header>
          <h1 className="font-display font-display-soft text-headline text-ink">Prune</h1>
          <p className="mt-1 text-label text-ink-mute">
            Find junk, review it in groups, stage reversibly. Deletion happens only in the
            reaper, after the grace period — and what it deletes goes to the trash, where it
            stays restorable.
          </p>
        </header>

        {status.data ? <StatusStrip status={status.data} /> : null}

        <TabsRoot
          value={tab}
          onValueChange={(next) =>
            void navigate({ search: { tab: next as PruneTab }, replace: true })
          }
        >
          <TabsList>
            <TabsTrigger value="triage">Triage</TabsTrigger>
            <TabsTrigger value="review">Review</TabsTrigger>
            <TabsTrigger value="clusters">Clusters</TabsTrigger>
            <TabsTrigger value="staged">
              Staged{status.data && status.data.staged > 0 ? ` · ${formatCount(status.data.staged)}` : ''}
            </TabsTrigger>
            <TabsTrigger value="trash">
              Trash{status.data && status.data.trash ? ` · ${formatCount(status.data.trash.items)}` : ''}
            </TabsTrigger>
            <TabsTrigger value="rules">Rules</TabsTrigger>
            <TabsTrigger value="history">History</TabsTrigger>
          </TabsList>
          <TabsContent value="triage" className="pt-4">
            <TriageTab
              onOpenBundle={(bundle) =>
                void navigate({
                  search: { tab: 'review', detector: bundle.detector ?? undefined },
                  replace: true,
                })
              }
            />
          </TabsContent>
          <TabsContent value="review" className="pt-4">
            <ReviewTab />
          </TabsContent>
          <TabsContent value="clusters" className="pt-4">
            <ClustersTab />
          </TabsContent>
          <TabsContent value="staged" className="pt-4">
            <StagedTab />
          </TabsContent>
          <TabsContent value="trash" className="pt-4">
            <TrashTab />
          </TabsContent>
          <TabsContent value="rules" className="pt-4">
            <RulesTab />
          </TabsContent>
          <TabsContent value="history" className="pt-4">
            <HistoryTab />
          </TabsContent>
        </TabsRoot>
      </div>
    </div>
  );
}

function StatusStrip({ status }: { status: PruneStatusResponse }) {
  const now = useNow();
  const reaper = status.reaper;

  const reaperValue = reaper.halted
    ? 'halted'
    : reaper.deferred > 0
      ? `deferred ${formatCount(reaper.deferred)}`
      : reaper.next_run_at
        ? `next in ${graceCountdown(reaper.next_run_at, now)}`
        : 'idle';
  const reaperCaption = reaper.halted
    ? (reaper.halted_reason ?? 'refusing to delete')
    : reaper.deferred > 0
      ? (reaper.deferred_reason ?? undefined)
      : `${formatCount(reaper.deleted_last_hour)} deleted last hour · limit ${formatCount(status.limits.max_docs_per_hour)}/h`;

  return (
    <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
      <Stat label="candidates open" value={formatCount(status.candidates)} />
      <Stat
        label="staged"
        value={formatCount(status.staged)}
        caption={
          status.soonest_expiry
            ? `soonest grace ends in ${graceCountdown(status.soonest_expiry, now)}`
            : undefined
        }
        tone={status.staged_expiring_24h > 0 ? 'gold' : 'default'}
      />
      <Stat label="deleted this week" value={formatCount(status.deleted_7d)} />
      <Stat
        label="reaper"
        value={reaperValue}
        caption={reaperCaption}
        tone={reaper.halted ? 'rose' : reaper.deferred > 0 ? 'gold' : 'default'}
      />
    </div>
  );
}
