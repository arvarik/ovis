/**
 * Acceptance sampling — deciding about a group without reading all of it.
 *
 * The backend draws a random sample server-side and states, in a sentence, what
 * accepting it would mean: with zero mistakes in `n` independent draws, the
 * true error rate is below `1 - (1 - c)^(1/n)` at confidence `c`. That sentence
 * is the point of the feature and it arrives from the server, so it is shown
 * verbatim rather than reassembled here from the numbers.
 *
 * The draw is server-side for a reason worth keeping visible in the UI: a
 * client that picked its own sample could pick an easy one. "Draw another"
 * refetches rather than re-filters.
 */
import { useQuery } from '@tanstack/react-query';
import { pruneSampleQuery } from '@/api/queries';
import type { PruneBundle } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Sheet } from '@/components/primitives/Sheet';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';

export function SampleSheet({ bundle, onClose }: { bundle: PruneBundle; onClose: () => void }) {
  // Scoped by reason code rather than by detector: a bundle *is* a reason code,
  // and several detectors can emit the same one.
  const sample = useQuery(pruneSampleQuery({ code: bundle.key, n: 40 }));

  return (
    <Sheet open onOpenChange={(open) => !open && onClose()} title={`Sample · ${bundle.title}`}>
      <div className="space-y-4">
        <p className="text-caption text-ink-mute">{bundle.description}</p>

        {sample.isPending ? <Skeleton className="h-40 w-full" /> : null}
        {sample.isError ? (
          <EmptyState
            title="Could not draw a sample"
            description={sample.error?.message ?? 'The sample could not be loaded.'}
          />
        ) : null}

        {sample.data ? (
          <>
            <div className="rounded-md border border-line bg-surface-2 p-3">
              <p className="text-label text-ink">{sample.data.statement}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <Badge tone="neutral">
                  {formatCount(sample.data.sample_size)} of {formatCount(sample.data.population)}
                </Badge>
                <Badge tone="neutral">{Math.round(sample.data.confidence * 100)}% confidence</Badge>
                <Badge tone={sample.data.max_error_rate > 0.1 ? 'gold' : 'mint'}>
                  ≤ {(sample.data.max_error_rate * 100).toFixed(1)}% error
                </Badge>
              </div>
            </div>

            <p className="text-caption text-ink-mute">
              Read these. If every one of them should go, the bound above is what you are accepting
              for the rest of the group. If even one should be kept, tighten this group&rsquo;s
              threshold instead of staging it.
            </p>

            {sample.data.documents.length === 0 ? (
              <EmptyState
                title="Nothing to sample"
                description="This group is empty — there are no open candidates carrying this reason."
              />
            ) : (
              <ol className="space-y-2">
                {sample.data.documents.map((doc, index) => (
                  <li key={doc.document_id} className="rounded-md border border-line p-3">
                    <div className="flex items-baseline gap-2">
                      <span className="text-caption tabular-nums text-ink-faint">{index + 1}</span>
                      <span className="text-ink">{doc.semantic_id ?? doc.document_id}</span>
                    </div>
                    <div className="mt-0.5 break-all text-caption text-ink-mute">
                      {doc.document_id}
                    </div>
                    <div className="mt-1 text-caption text-ink-mute">
                      {doc.chunk_count === null
                        ? 'chunks not counted yet'
                        : `${formatCount(doc.chunk_count)} chunk${doc.chunk_count === 1 ? '' : 's'}`}
                      {doc.signals.length > 0 ? ` · ${doc.signals.join(', ')}` : ''}
                    </div>
                  </li>
                ))}
              </ol>
            )}

            <div className="flex items-center gap-2 border-t border-line pt-3">
              <Button
                variant="secondary"
                onClick={() => void sample.refetch()}
                disabled={sample.isFetching}
              >
                {sample.isFetching ? 'Drawing…' : 'Draw another'}
              </Button>
              <p className="text-caption text-ink-mute">
                Drawn server-side, so a fresh draw is genuinely random rather than a re-shuffle of
                the same rows.
              </p>
            </div>
          </>
        ) : null}
      </div>
    </Sheet>
  );
}
