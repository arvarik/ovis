import { useState } from 'react';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Braces, Copy } from 'lucide-react';
import { toast } from 'sonner';
import { chunkVectorQuery, pageChunksQuery } from '@/api/queries';
import { count as formatCount } from '@/lib/format';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Dialog } from '@/components/primitives/Dialog';
import { ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { Spinner } from '@/components/primitives/Spinner';

/**
 * The vector dialog shows one REAL vector fetched from the index (D3 fix —
 * no fabricated floats, dim and model from the response, copyable).
 */
function VectorDialog({
  docId,
  chunkIndex,
  onClose,
}: {
  docId: string;
  chunkIndex: number;
  onClose: () => void;
}) {
  const vector = useQuery(chunkVectorQuery(docId, chunkIndex));
  return (
    <Dialog
      open
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
      title={`Chunk ${chunkIndex} vector`}
      description={vector.data ? `${vector.data.model} · ${vector.data.dim} dimensions` : undefined}
    >
      {vector.isPending ? (
        <div className="flex justify-center py-8">
          <Spinner label="Fetching the real vector from OpenSearch…" />
        </div>
      ) : vector.isError ? (
        <ErrorState error={vector.error} title="Vector could not load" onRetry={() => void vector.refetch()} />
      ) : (
        <div className="space-y-3">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              void navigator.clipboard.writeText(JSON.stringify(vector.data.vector));
              toast(`${vector.data.dim}-dim vector copied`);
            }}
          >
            <Copy className="size-4" aria-hidden /> Copy all {vector.data.dim} floats
          </Button>
          <div className="max-h-72 overflow-y-auto rounded-lg border border-line bg-well p-3 font-mono text-caption break-all text-ink-mute">
            [{vector.data.vector.map((v) => v.toFixed(5)).join(', ')}]
          </div>
        </div>
      )}
    </Dialog>
  );
}

export function ChunksTab({ docId }: { docId: string }) {
  const chunks = useInfiniteQuery(pageChunksQuery(docId));
  const [vectorFor, setVectorFor] = useState<number | null>(null);

  if (chunks.isPending)
    return (
      <div className="space-y-3" aria-hidden>
        {Array.from({ length: 3 }, (_, i) => (
          <Skeleton key={i} className="h-28 rounded-xl" />
        ))}
      </div>
    );
  if (chunks.isError)
    return <ErrorState error={chunks.error} title="Chunks could not load" onRetry={() => void chunks.refetch()} />;

  const first = chunks.data.pages[0];
  const items = chunks.data.pages.flatMap((p) => p.items);

  return (
    <div className="space-y-3">
      <p className="font-mono text-caption text-ink-faint">
        {formatCount(first?.total_chunks ?? 0)} chunks in the index · {first?.embedding_model} ·{' '}
        {first?.embedding_dim}d
      </p>

      {items.map((chunk) => (
        <Card key={chunk.chunk_index} className="space-y-2">
          <div className="flex items-center justify-between gap-2">
            <span className="font-mono text-mono-sm text-ink">#{chunk.chunk_index}</span>
            <div className="flex items-center gap-2">
              {chunk.token_estimate !== null ? (
                <span
                  className="font-mono text-caption text-ink-faint"
                  title="Word-count heuristic, not a tokeniser result"
                >
                  ~{formatCount(chunk.token_estimate)} words
                </span>
              ) : null}
              <Button variant="secondary" size="sm" onClick={() => setVectorFor(chunk.chunk_index)}>
                <Braces className="size-3.5" aria-hidden /> Load vector
              </Button>
            </div>
          </div>
          {chunk.title && chunk.title !== '' ? (
            <p className="text-label font-medium text-ink-mute">{chunk.title}</p>
          ) : null}
          <p className="line-clamp-4 text-body break-words text-ink-mute">
            {chunk.blurb ?? chunk.content ?? '—'}
          </p>
        </Card>
      ))}

      {chunks.hasNextPage ? (
        <div className="flex justify-center">
          <Button
            variant="secondary"
            onClick={() => void chunks.fetchNextPage()}
            disabled={chunks.isFetchingNextPage}
          >
            {chunks.isFetchingNextPage ? 'Loading…' : `Load more (${formatCount(items.length)} of ${formatCount(first?.total_chunks ?? 0)})`}
          </Button>
        </div>
      ) : null}

      {vectorFor !== null ? (
        <VectorDialog docId={docId} chunkIndex={vectorFor} onClose={() => setVectorFor(null)} />
      ) : null}
    </div>
  );
}
