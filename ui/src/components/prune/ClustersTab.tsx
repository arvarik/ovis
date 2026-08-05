/**
 * Clusters — duplicate groups reviewed whole, one screen at a time.
 *
 * 49,683 hash groups is a reviewable number of decisions. The 184,058
 * individual candidates inside them are not. So the unit here is the cluster:
 * the keeper is pinned first with the rule that chose it, every other member is
 * shown against it, and one action stages the rest of the group.
 *
 * Keyboard-first, because the whole point is doing this many times in a row:
 * j/k move between clusters, a stages the non-keepers, s skips.
 */
import { useCallback, useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { pruneClustersQuery } from '@/api/queries';
import { usePruneStage } from '@/api/mutations';
import type { PruneCluster, PruneClusterMember } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Kbd } from '@/components/primitives/Kbd';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';

export function ClustersTab() {
  const [method, setMethod] = useState('hash');
  const [index, setIndex] = useState(0);
  const clusters = useQuery(pruneClustersQuery(method, 25));
  const stage = usePruneStage();

  const items = clusters.data?.items ?? [];
  // Clamped at render rather than corrected in an effect: staging a cluster
  // shortens the list, and writing the correction back as state would cost a
  // second render for no gain.
  const safeIndex = Math.min(index, Math.max(items.length - 1, 0));
  const current = items[safeIndex];

  const stageCluster = useCallback(
    (cluster: PruneCluster) => {
      const ids = cluster.members
        .filter((m) => !m.is_keeper && m.candidate_id !== null)
        .map((m) => m.candidate_id as number);
      if (ids.length === 0) return;
      stage.mutate({ ids, confirm_count: ids.length }, { onSuccess: () => setIndex((i) => i + 1) });
    },
    [stage],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) {
        return;
      }
      if (event.key === 'j' || event.key === 's') {
        setIndex((i) => Math.min(i + 1, items.length - 1));
      }
      if (event.key === 'k') setIndex((i) => Math.max(i - 1, 0));
      if (event.key === 'a' && current) stageCluster(current);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [current, items.length, stageCluster]);

  if (clusters.isPending) return <Skeleton className="h-64 w-full" />;
  if (clusters.isError) {
    return (
      <EmptyState
        title="Clusters unavailable"
        description={clusters.error?.message ?? 'The clusters could not be loaded.'}
      />
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Select
            value={method}
            onValueChange={(next) => {
              setMethod(next);
              setIndex(0);
            }}
            ariaLabel="Group duplicates by"
            options={[
              { value: 'hash', label: 'Identical content' },
              { value: 'url', label: 'Same canonical URL' },
            ]}
          />
        </div>
        <div className="flex items-center gap-2 text-caption text-ink-mute">
          <Kbd>j</Kbd> next <Kbd>k</Kbd> previous <Kbd>a</Kbd> stage the copies <Kbd>s</Kbd> skip
        </div>
      </div>

      {items.length === 0 ? (
        <EmptyState
          title="No duplicate clusters"
          description={
            method === 'hash'
              ? 'Run an exact_duplicate scan to group documents by identical content.'
              : 'Run a url_variant scan to group documents by canonical URL.'
          }
        />
      ) : current ? (
        <>
          <div className="flex items-center justify-between text-label text-ink-mute">
            <span>
              Cluster {safeIndex + 1} of {items.length}
            </span>
            <span>{formatCount(current.size)} documents in this group</span>
          </div>
          <ClusterCard
            cluster={current}
            onStage={() => stageCluster(current)}
            onSkip={() => setIndex((i) => Math.min(i + 1, items.length - 1))}
            staging={stage.isPending}
          />
        </>
      ) : null}
    </div>
  );
}

function ClusterCard({
  cluster,
  onStage,
  onSkip,
  staging,
}: {
  cluster: PruneCluster;
  onStage: () => void;
  onSkip: () => void;
  staging: boolean;
}) {
  const keeper = cluster.members.find((m) => m.is_keeper);
  const copies = cluster.members.filter((m) => !m.is_keeper);

  return (
    <Card className="space-y-4 p-4">
      {keeper ? (
        <div className="rounded-md border border-mint/30 bg-mint/10 p-3">
          <div className="flex items-center gap-2">
            <Badge tone="mint">keeping</Badge>
            <span className="text-caption text-ink-mute">{cluster.keeper_reason}</span>
          </div>
          <div className="mt-1 break-all text-ink">{keeper.link ?? keeper.document_id}</div>
          <MemberMeta member={keeper} />
        </div>
      ) : null}

      <div>
        <div className="text-label text-ink-mute">
          {formatCount(copies.length)} cop{copies.length === 1 ? 'y' : 'ies'} to stage
        </div>
        <ul className="mt-2 space-y-2">
          {copies.map((member) => (
            <li key={member.document_id} className="rounded-md border border-line p-3">
              <div className="break-all text-ink">{member.link ?? member.document_id}</div>
              <MemberMeta member={member} keeper={keeper} />
            </li>
          ))}
        </ul>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-line pt-3">
        <Button onClick={onStage} disabled={staging || copies.length === 0}>
          {staging ? 'Staging…' : `Stage ${formatCount(copies.length)} and keep 1`}
        </Button>
        <Button variant="secondary" onClick={onSkip}>
          Skip
        </Button>
        <p className="text-caption text-ink-mute">
          Staging hides them from search and starts the grace period. Nothing is deleted yet,
          and restoring is one click until the reaper runs.
        </p>
      </div>
    </Card>
  );
}

/**
 * Each member is described against the keeper rather than in isolation — the
 * question a reviewer is answering is "is this the same page?", and a chunk
 * count on its own does not answer it.
 */
function MemberMeta({
  member,
  keeper,
}: {
  member: PruneClusterMember;
  keeper?: PruneClusterMember;
}) {
  const chunkDelta =
    keeper && member.chunk_count !== null && keeper.chunk_count !== null
      ? member.chunk_count - keeper.chunk_count
      : null;

  return (
    <div className="mt-1 flex flex-wrap gap-x-3 text-caption text-ink-mute">
      <span>{member.semantic_id ?? '—'}</span>
      <span>
        {member.chunk_count ?? '—'} chunks
        {chunkDelta !== null && chunkDelta !== 0
          ? ` (${chunkDelta > 0 ? '+' : ''}${chunkDelta} vs keeper)`
          : ''}
      </span>
      {member.updated_at ? (
        <span>updated {new Date(member.updated_at).toLocaleDateString()}</span>
      ) : null}
    </div>
  );
}
