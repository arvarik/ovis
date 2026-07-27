/**
 * The waiting room: everything hidden-but-intact, sorted by how soon its
 * grace ends. Restore is instant and needs no confirmation (the safe
 * direction); "Delete sooner" is still reaper-executed.
 */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Archive } from 'lucide-react';
import { pruneCandidatesQuery, pruneStatusQuery } from '@/api/queries';
import { usePruneRestore, usePruneScheduleDelete } from '@/api/mutations';
import type { PruneCandidateItem } from '@/api/types';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Checkbox } from '@/components/primitives/Checkbox';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';
import { CandidateSheet } from './CandidateSheet';
import { PruneConfirmDialog } from './PruneConfirmDialog';
import { chunkLabel, documentLabel, graceCountdown, ReasonChips, RiskBadge, useNow } from './pruneShared';

const PAGE_SIZE = 50;

export function StagedTab() {
  const status = useQuery(pruneStatusQuery);
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [openCandidate, setOpenCandidate] = useState<number | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const now = useNow();

  const staged = useQuery(
    pruneCandidatesQuery({ state: 'staged', sort: 'expiry_asc', limit: PAGE_SIZE, page }),
  );
  const restore = usePruneRestore();
  const scheduleDelete = usePruneScheduleDelete();

  const items = staged.data?.items ?? [];
  const selectedItems = items.filter((item) => selected.has(item.id));
  const limits = status.data?.limits;

  if (staged.isError) {
    return <ErrorState error={staged.error} onRetry={() => void staged.refetch()} />;
  }

  return (
    <div className="space-y-4">
      <p className="rounded-lg border border-gold/30 bg-gold/5 px-3 py-2 text-label text-ink">
        Staged documents are hidden from Onyx search but fully intact; they delete automatically
        when their grace ends.
      </p>

      <Card className="space-y-3">
        {staged.isPending ? (
          <div className="space-y-2">
            <Skeleton className="h-10" />
            <Skeleton className="h-10" />
          </div>
        ) : items.length === 0 ? (
          <EmptyState
            icon={<Archive aria-hidden />}
            title="Nothing is staged"
            description="Stage candidates from the Review tab. They wait here — restorable — until their grace ends."
          />
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2 text-label text-ink-mute">
              <Checkbox
                checked={selectedItems.length === items.length && items.length > 0}
                onCheckedChange={(checked) =>
                  setSelected(checked ? new Set(items.map((i) => i.id)) : new Set())
                }
                label="Select every staged row on this page"
              />
              <span>
                {selectedItems.length > 0
                  ? `${selectedItems.length} selected`
                  : `${formatCount(staged.data.total)} staged`}
              </span>
              {selectedItems.length > 0 ? (
                <span className="flex items-center gap-2">
                  <Button
                    size="sm"
                    disabled={restore.isPending}
                    onClick={() =>
                      restore.mutate(
                        { ids: selectedItems.map((i) => i.id) },
                        { onSuccess: () => setSelected(new Set()) },
                      )
                    }
                  >
                    Restore selected
                  </Button>
                  <Button size="sm" variant="destructive" onClick={() => setConfirmingDelete(true)}>
                    Delete sooner…
                  </Button>
                </span>
              ) : null}
            </div>

            <ul className="divide-y divide-line">
              {items.map((item) => (
                <StagedRow
                  key={item.id}
                  item={item}
                  now={now}
                  checked={selected.has(item.id)}
                  onCheck={(checked) =>
                    setSelected((prev) => {
                      const next = new Set(prev);
                      if (checked) next.add(item.id);
                      else next.delete(item.id);
                      return next;
                    })
                  }
                  onOpen={() => setOpenCandidate(item.id)}
                  onRestore={() => restore.mutate({ ids: [item.id] })}
                  restorePending={restore.isPending}
                />
              ))}
            </ul>

            <div className="flex items-center justify-between text-label text-ink-mute">
              <span>page {page}</span>
              <span className="flex gap-2">
                <Button size="sm" disabled={page <= 1} onClick={() => setPage(page - 1)}>
                  Previous
                </Button>
                <Button size="sm" disabled={!staged.data.has_more} onClick={() => setPage(page + 1)}>
                  Next
                </Button>
              </span>
            </div>
          </>
        )}
      </Card>

      <CandidateSheet
        candidateId={openCandidate}
        onOpenChange={(open) => {
          if (!open) setOpenCandidate(null);
        }}
      />

      {limits ? (
        <PruneConfirmDialog
          open={confirmingDelete}
          onOpenChange={setConfirmingDelete}
          verb="Schedule deletion of"
          total={selectedItems.length}
          chunkSum={selectedItems.reduce((sum, i) => sum + (i.chunk_count ?? 0), 0)}
          chunkSumComplete
          riskyCount={selectedItems.filter((i) => i.recrawl_risk).length}
          bigBatch={limits.big_batch}
          graceDays={limits.grace_days}
          consequence="Their deadlines move to now; the reaper deletes at its next cycle. Restore works until the moment it runs."
          confirmLabel="Schedule deletion"
          destructive
          pending={scheduleDelete.isPending}
          onConfirm={() =>
            scheduleDelete.mutate(
              { ids: selectedItems.map((i) => i.id), confirm_count: selectedItems.length },
              {
                onSuccess: () => {
                  setConfirmingDelete(false);
                  setSelected(new Set());
                },
              },
            )
          }
        />
      ) : null}
    </div>
  );
}

function StagedRow({
  item,
  now,
  checked,
  onCheck,
  onOpen,
  onRestore,
  restorePending,
}: {
  item: PruneCandidateItem;
  now: number;
  checked: boolean;
  onCheck: (checked: boolean) => void;
  onOpen: () => void;
  onRestore: () => void;
  restorePending: boolean;
}) {
  const countdown = item.stage_expires_at ? graceCountdown(item.stage_expires_at, now) : '—';
  return (
    <li className="flex min-h-11 items-center gap-3 px-1 py-2 transition-colors hover:bg-hover">
      <Checkbox checked={checked} onCheckedChange={onCheck} label={`Select ${item.document_id}`} />
      <button
        type="button"
        onClick={onOpen}
        className="min-w-0 flex-1 text-left outline-none focus-visible:rounded focus-visible:ring-1 focus-visible:ring-gold/50"
      >
        <p className="truncate text-label text-ink">{documentLabel(item)}</p>
        <span className="mt-0.5 flex flex-wrap items-center gap-1.5 text-caption text-ink-mute">
          <ReasonChips reasons={item.reasons} />
          <span>{chunkLabel(item.chunk_count)} chunks</span>
          {item.prev_hidden ? <span>was already hidden</span> : null}
          <RiskBadge item={item} />
        </span>
      </button>
      <span
        className={
          countdown === 'due now'
            ? 'shrink-0 font-mono text-caption text-rose'
            : 'shrink-0 font-mono text-caption text-gold'
        }
      >
        {countdown}
      </span>
      <Button size="sm" disabled={restorePending} onClick={onRestore}>
        Restore
      </Button>
    </li>
  );
}
