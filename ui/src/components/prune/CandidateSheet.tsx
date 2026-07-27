/**
 * The candidate drawer: reasons first (the Inspector philosophy — evidence
 * before actions), duplicate pairs side by side with the keeper labeled,
 * lifecycle actions in the footer per state.
 */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { pruneCandidateQuery, pruneStatusQuery } from '@/api/queries';
import {
  usePruneDismiss,
  usePruneRestore,
  usePruneScheduleDelete,
  usePruneStage,
} from '@/api/mutations';
import type { PruneCandidateDetail } from '@/api/types';
import { Badge, statusTone } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Checkbox } from '@/components/primitives/Checkbox';
import { Sheet } from '@/components/primitives/Sheet';
import { Skeleton } from '@/components/primitives/Skeleton';
import { absolute, relative } from '@/lib/format';
import { PruneConfirmDialog } from './PruneConfirmDialog';
import { chunkLabel, graceCountdown, ReasonChips, RiskBadge, useNow } from './pruneShared';

export function CandidateSheet({
  candidateId,
  onOpenChange,
}: {
  candidateId: number | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = candidateId !== null;
  return (
    <Sheet open={open} onOpenChange={onOpenChange} title="Prune candidate">
      {candidateId !== null ? <SheetBody candidateId={candidateId} onClose={() => onOpenChange(false)} /> : null}
    </Sheet>
  );
}

function SheetBody({ candidateId, onClose }: { candidateId: number; onClose: () => void }) {
  const detail = useQuery(pruneCandidateQuery(candidateId));
  if (detail.isPending) {
    return (
      <div className="space-y-3 p-5">
        <Skeleton className="h-6 w-2/3" />
        <Skeleton className="h-24" />
        <Skeleton className="h-24" />
      </div>
    );
  }
  if (detail.isError || !detail.data) {
    return <p className="p-5 text-label text-rose">This candidate could not be loaded.</p>;
  }
  return <CandidateBody detail={detail.data} onClose={onClose} />;
}

function CandidateBody({ detail, onClose }: { detail: PruneCandidateDetail; onClose: () => void }) {
  const now = useNow();
  const status = useQuery(pruneStatusQuery);
  const stage = usePruneStage();
  const dismiss = usePruneDismiss();
  const restore = usePruneRestore();
  const scheduleDelete = usePruneScheduleDelete();
  const [confirming, setConfirming] = useState<'stage' | 'delete' | null>(null);
  const [excludeFuture, setExcludeFuture] = useState(false);

  const limits = status.data?.limits;
  const item = detail;

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-line px-5 py-4">
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={statusTone(item.state)}>{item.state}</Badge>
          <RiskBadge item={item} />
          {item.excluded ? <Badge tone="teal">on the exclusion list</Badge> : null}
          {!item.doc_exists ? <Badge tone="rose">document row gone</Badge> : null}
        </div>
        <h2 className="mt-2 break-all font-display font-display-soft text-title text-ink">
          {item.semantic_id ?? item.document_id}
        </h2>
        <p className="mt-1 break-all font-mono text-caption text-ink-faint">{item.document_id}</p>
        <p className="mt-1 text-label text-ink-mute">
          {item.connector_name ?? 'no connector'} · {chunkLabel(item.chunk_count)} chunks
          {item.chunk_count === null ? ' (not counted yet)' : ''} · confidence{' '}
          {item.confidence.toFixed(2)}
        </p>
      </header>

      <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-5 py-4">
        <section>
          <h3 className="text-label font-medium text-ink">Reasons</h3>
          <ul className="mt-2 space-y-2">
            {item.reasons.map((reason) => (
              <li
                key={`${reason.detector}:${reason.code}`}
                className="rounded-lg border border-line bg-surface/60 p-3"
              >
                <div className="flex items-center gap-2">
                  <ReasonChips reasons={[reason]} />
                  <span className="font-mono text-caption text-ink-faint">
                    confidence {reason.confidence.toFixed(2)}
                  </span>
                </div>
                <p className="mt-1.5 text-label text-ink-mute">{reason.detail}</p>
              </li>
            ))}
          </ul>
        </section>

        {detail.pair ? (
          <section>
            <h3 className="text-label font-medium text-ink">
              Duplicate pair · {(detail.pair.similarity * 100).toFixed(1)}% similar
            </h3>
            <div className="mt-2 grid gap-2 md:grid-cols-2">
              <div className="rounded-lg border border-line bg-surface/60 p-3">
                <Badge tone="rose">this candidate</Badge>
                <p className="mt-2 break-all text-label text-ink">{item.semantic_id ?? '—'}</p>
                <p className="mt-1 break-all font-mono text-caption text-ink-faint">
                  {item.document_id}
                </p>
                <p className="mt-1 text-caption text-ink-mute">
                  {chunkLabel(item.chunk_count)} chunks
                </p>
              </div>
              <div className="rounded-lg border border-mint/30 bg-mint/5 p-3">
                <Badge tone="mint">keeper</Badge>
                {detail.pair.kept ? (
                  <>
                    <p className="mt-2 break-all text-label text-ink">
                      {detail.pair.kept.semantic_id}
                    </p>
                    <p className="mt-1 break-all font-mono text-caption text-ink-faint">
                      {detail.pair.kept_id}
                    </p>
                    <p className="mt-1 text-caption text-ink-mute">
                      {chunkLabel(detail.pair.kept.chunk_count)} chunks · updated{' '}
                      {relative(detail.pair.kept.updated_at)}
                    </p>
                    <Link
                      to="/pages/$docId"
                      params={{ docId: detail.pair.kept_id }}
                      className="mt-1 inline-block text-caption text-gold hover:underline"
                    >
                      open keeper →
                    </Link>
                  </>
                ) : (
                  <p className="mt-2 break-all font-mono text-caption text-ink-mute">
                    {detail.pair.kept_id} (no longer exists)
                  </p>
                )}
              </div>
            </div>
          </section>
        ) : null}

        {item.state === 'staged' ? (
          <section className="rounded-lg border border-gold/30 bg-gold/5 p-3 text-label">
            <p className="text-ink">
              Hidden from search since {item.staged_at ? absolute(item.staged_at) : '—'} · grace
              ends in{' '}
              <span className="font-medium text-gold">
                {item.stage_expires_at ? graceCountdown(item.stage_expires_at, now) : '—'}
              </span>
            </p>
            {item.prev_hidden ? (
              <p className="mt-1 text-ink-mute">
                This document was already hidden before staging; restore returns it to
                hidden-but-unstaged.
              </p>
            ) : null}
          </section>
        ) : null}

        {item.doc_exists ? (
          <Link
            to="/pages/$docId"
            params={{ docId: item.document_id }}
            className="inline-block text-label text-gold hover:underline"
          >
            open this document in the Explorer →
          </Link>
        ) : null}
      </div>

      {item.state === 'candidate' || item.state === 'staged' ? (
        <footer className="space-y-2 border-t border-line px-5 py-3">
          {item.state === 'candidate' ? (
            <>
              <label className="flex items-center gap-2 text-label text-ink-mute">
                <Checkbox
                  checked={excludeFuture}
                  onCheckedChange={setExcludeFuture}
                  label="Never flag this document again"
                />
                never flag this document again when dismissing
              </label>
              <div className="flex gap-2">
                <Button
                  className="flex-1"
                  disabled={dismiss.isPending}
                  onClick={() =>
                    dismiss.mutate(
                      { ids: [item.id], exclude_future: excludeFuture },
                      { onSuccess: onClose },
                    )
                  }
                >
                  Dismiss
                </Button>
                <Button
                  className="flex-1"
                  variant="primary"
                  onClick={() => setConfirming('stage')}
                >
                  Stage
                </Button>
              </div>
            </>
          ) : (
            <div className="flex gap-2">
              <Button
                className="flex-1"
                disabled={restore.isPending}
                onClick={() => restore.mutate({ ids: [item.id] }, { onSuccess: onClose })}
              >
                Restore
              </Button>
              <Button
                className="flex-1"
                variant="destructive"
                onClick={() => setConfirming('delete')}
              >
                Delete sooner…
              </Button>
            </div>
          )}
        </footer>
      ) : null}

      {limits ? (
        <>
          <PruneConfirmDialog
            open={confirming === 'stage'}
            onOpenChange={(open) => setConfirming(open ? 'stage' : null)}
            verb="Stage"
            total={1}
            chunkSum={item.chunk_count ?? 0}
            chunkSumComplete={item.chunk_count !== null}
            riskyCount={item.recrawl_risk ? 1 : 0}
            bigBatch={limits.big_batch}
            graceDays={limits.grace_days}
            consequence="Staged documents are hidden from Onyx search but fully intact; they delete automatically when their grace ends."
            confirmLabel="Stage — hide from search"
            pending={stage.isPending}
            onConfirm={() =>
              stage.mutate(
                { ids: [item.id], confirm_count: 1 },
                {
                  onSuccess: () => {
                    setConfirming(null);
                    onClose();
                  },
                },
              )
            }
          />
          <PruneConfirmDialog
            open={confirming === 'delete'}
            onOpenChange={(open) => setConfirming(open ? 'delete' : null)}
            verb="Schedule deletion of"
            total={1}
            chunkSum={item.chunk_count ?? 0}
            chunkSumComplete={item.chunk_count !== null}
            riskyCount={item.recrawl_risk ? 1 : 0}
            bigBatch={limits.big_batch}
            graceDays={limits.grace_days}
            consequence="The deadline moves to now; the reaper deletes at its next cycle. Restore works until the moment it runs."
            confirmLabel="Schedule deletion"
            destructive
            pending={scheduleDelete.isPending}
            onConfirm={() =>
              scheduleDelete.mutate(
                { ids: [item.id], confirm_count: 1 },
                {
                  onSuccess: () => {
                    setConfirming(null);
                    onClose();
                  },
                },
              )
            }
          />
        </>
      ) : null}
    </div>
  );
}
