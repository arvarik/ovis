/**
 * The never-flag list, and the way back off it.
 *
 * Dismissing with "never flag again" writes a row here, and until now that was
 * a one-way door from the UI: the list had no screen, so an exclusion added by
 * a mis-click was invisible and permanent. It is the one place in pruning where
 * *not* acting is the dangerous direction — an excluded document is one no scan
 * will ever mention again, so its absence from review looks exactly like it
 * being fine.
 *
 * Removing an exclusion is a safe direction and behaves like one: no
 * confirmation, and the toast says plainly that nothing happened to the
 * document itself.
 */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { ShieldOff } from 'lucide-react';
import { pruneExclusionsQuery } from '@/api/queries';
import { usePruneExclusionRemove } from '@/api/mutations';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount, relative } from '@/lib/format';

const PAGE_SIZE = 25;

/**
 * Why a document is on the list, in words rather than the stored code. The two
 * cases mean genuinely different things — one says "this was never junk", the
 * other says "this was deleted and must stay gone" — and only the second one
 * makes the reaper re-stage a recrawled copy.
 */
function describeReason(reason: string): { label: string; tone: 'gold' | 'neutral' } {
  switch (reason) {
    case 'user_excluded':
      return { label: 'dismissed as not junk', tone: 'neutral' };
    case 'deleted_with_remember':
      return { label: 'deleted with remember', tone: 'gold' };
    default:
      return { label: reason.replace(/_/g, ' '), tone: 'neutral' };
  }
}

export function ExclusionsCard() {
  const [page, setPage] = useState(1);
  const exclusions = useQuery(pruneExclusionsQuery({ limit: PAGE_SIZE, page }));
  const remove = usePruneExclusionRemove();

  if (exclusions.isError) {
    return <ErrorState error={exclusions.error} onRetry={() => void exclusions.refetch()} />;
  }

  const items = exclusions.data?.items ?? [];
  const total = exclusions.data?.total ?? 0;

  return (
    <Card className="space-y-3">
      <div>
        <h2 className="font-display font-display-soft text-title text-ink">
          Never flag these
          {total > 0 ? <span className="ml-2 text-label text-ink-mute">{formatCount(total)}</span> : null}
        </h2>
        <p className="mt-1 text-label text-ink-mute">
          Documents no scan will ever raise again. Added by dismissing with &ldquo;never flag
          again&rdquo;, or by deleting with remember — in which case the reaper also re-stages the
          document if the crawler brings it back.
        </p>
      </div>

      {exclusions.isPending ? (
        <div className="space-y-2">
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
        </div>
      ) : items.length === 0 ? (
        <EmptyState
          icon={<ShieldOff aria-hidden />}
          title="Nothing is excluded"
          description="Every document in the corpus is still eligible to be flagged by a scan."
        />
      ) : (
        <ul className="divide-y divide-line">
          {items.map((item) => {
            const reason = describeReason(item.reason);
            return (
              <li key={item.document_id} className="flex flex-wrap items-center gap-2 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-label text-ink" title={item.document_id}>
                    {item.document_id}
                  </p>
                  <p className="mt-0.5 flex flex-wrap items-center gap-2 text-caption text-ink-mute">
                    <Badge tone={reason.tone}>{reason.label}</Badge>
                    {item.note ? <span className="font-mono">{item.note}</span> : null}
                    <span>{relative(item.created_at)}</span>
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={remove.isPending}
                  onClick={() => remove.mutate(item.document_id)}
                >
                  Allow flagging again
                </Button>
              </li>
            );
          })}
        </ul>
      )}

      {total > PAGE_SIZE ? (
        <div className="flex items-center justify-between border-t border-line pt-2">
          <Button
            size="sm"
            variant="secondary"
            disabled={page === 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            Previous
          </Button>
          <span className="text-caption text-ink-mute">
            {formatCount(total)} excluded
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={!exclusions.data?.has_more}
            onClick={() => setPage((p) => p + 1)}
          >
            Next
          </Button>
        </div>
      ) : null}
    </Card>
  );
}
