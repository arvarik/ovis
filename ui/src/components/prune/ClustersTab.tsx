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
import { useCallback, useEffect, useMemo, useState } from 'react';
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
import { NarrateButton } from '@/components/prune/NarrateButton';
import { NarrationNote } from '@/components/prune/NarrationNote';
import { count as formatCount } from '@/lib/format';

/**
 * The copies this cluster can actually stage.
 *
 * A cluster is built from the *measurements* a scan recorded, so a member only
 * has a candidate row if a detector or a policy flagged it — dismiss one, or
 * browse clusters before committing a policy, and some copies have nothing to
 * act on. Staging is driven by candidate ids, so this is the number the button
 * has to be labelled and disabled by; counting all the non-keepers made it
 * promise more than it did, and promise something when it could do nothing.
 */
function stageableIds(cluster: PruneCluster): number[] {
  return cluster.members
    .filter((m) => !m.is_keeper && m.candidate_id !== null)
    .map((m) => m.candidate_id as number);
}

const PAGE_SIZE = 25;

export function ClustersTab() {
  const [method, setMethod] = useState('hash');
  const [index, setIndex] = useState(0);
  /**
   * Keys already paged past, newest last. The server pages clusters by key
   * cursor, so walking back is a matter of popping this rather than asking for
   * a page number the API does not have.
   */
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors.at(-1);
  const clusters = useQuery(pruneClustersQuery(method, PAGE_SIZE, after));
  const stage = usePruneStage();

  // Memoised because `nextPage` closes over it: a fresh `[]` on every render
  // would rebuild the callback, and with it the keydown listener, every time.
  const items = useMemo(() => clusters.data?.items ?? [], [clusters.data]);
  // A full page means there is very likely another; the API returns clusters,
  // not a total, so this is the honest signal available.
  const mayHaveMore = items.length === PAGE_SIZE;
  // Clamped at render rather than corrected in an effect: staging a cluster
  // shortens the list, and writing the correction back as state would cost a
  // second render for no gain.
  const safeIndex = Math.min(index, Math.max(items.length - 1, 0));
  const current = items[safeIndex];

  const stageCluster = useCallback(
    (cluster: PruneCluster) => {
      const ids = stageableIds(cluster);
      if (ids.length === 0) return;
      stage.mutate({ ids, confirm_count: ids.length }, { onSuccess: () => setIndex((i) => i + 1) });
    },
    [stage],
  );

  // Paging is part of the review flow, not a separate act: running off the end
  // of a page with `j` fetches the next one, because the alternative is
  // reviewing 25 clusters and quietly stopping.
  const nextPage = useCallback(() => {
    const last = items.at(-1);
    if (!mayHaveMore || !last) return;
    setCursors((prev) => [...prev, last.key]);
    setIndex(0);
  }, [items, mayHaveMore]);

  const prevPage = useCallback(() => {
    if (cursors.length === 0) return;
    setCursors((prev) => prev.slice(0, -1));
    setIndex(PAGE_SIZE); // clamped to the last cluster of that page at render
  }, [cursors.length]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) {
        return;
      }
      if (event.key === 'j' || event.key === 's') {
        if (index >= items.length - 1) nextPage();
        else setIndex((i) => Math.min(i + 1, items.length - 1));
      }
      if (event.key === 'k') {
        if (index === 0) prevPage();
        else setIndex((i) => Math.max(i - 1, 0));
      }
      if (event.key === 'a' && current) stageCluster(current);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [current, index, items.length, nextPage, prevPage, stageCluster]);

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
              setCursors([]);
            }}
            ariaLabel="Group duplicates by"
            options={[
              { value: 'hash', label: 'Identical content' },
              { value: 'url', label: 'Same canonical URL' },
            ]}
          />
          <NarrateButton subjectKind="cluster" method={method} disabled={items.length === 0} />
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
          <div className="flex flex-wrap items-center justify-between gap-2 text-label text-ink-mute">
            <span>
              Cluster {safeIndex + 1} of {items.length}
              {cursors.length > 0 ? ` · page ${cursors.length + 1}` : ''}
            </span>
            <span>{formatCount(current.size)} documents in this group</span>
          </div>
          <ClusterCard
            cluster={current}
            onStage={() => stageCluster(current)}
            onSkip={() =>
              safeIndex >= items.length - 1
                ? nextPage()
                : setIndex((i) => Math.min(i + 1, items.length - 1))
            }
            staging={stage.isPending}
          />
          <div className="flex items-center justify-between">
            <Button
              size="sm"
              variant="secondary"
              disabled={cursors.length === 0 || clusters.isFetching}
              onClick={prevPage}
            >
              Previous page
            </Button>
            <span className="text-caption text-ink-mute">
              {/* Clusters are keyed, not numbered — the server pages by cursor,
                  so a total would be a second full aggregate for a line of text. */}
              {mayHaveMore ? 'more groups after these' : 'last page'}
            </span>
            <Button
              size="sm"
              variant="secondary"
              disabled={!mayHaveMore || clusters.isFetching}
              onClick={nextPage}
            >
              Next page
            </Button>
          </div>
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
  const stageable = stageableIds(cluster).length;
  const unflagged = copies.length - stageable;

  return (
    <Card className="space-y-4 p-4">
      {cluster.narration ? <NarrationNote narration={cluster.narration} /> : null}
      {keeper ? (
        <div className="rounded-md border border-mint/40 bg-surface-2 p-3">
          <div className="flex items-center gap-2">
            <Badge tone="mint">keeping</Badge>
            <span className="text-caption text-ink-mute">{cluster.keeper_reason}</span>
          </div>
          <div className="mt-1 break-all text-ink">{keeper.link ?? keeper.document_id}</div>
          <MemberMeta member={keeper} />
        </div>
      ) : (
        /* A document carries one group at a time, so a page that is both a
           content duplicate and a URL variant appears in only one of the two
           clusters — and the other is then left without the document it would
           keep. Staging blind is exactly the mistake this screen exists to
           prevent, so say so rather than showing an empty space. */
        <div className="rounded-md border border-gold/40 bg-surface-2 p-3">
          <div className="flex items-center gap-2">
            <Badge tone="gold">no keeper shown</Badge>
            <span className="text-caption text-ink-mute">{cluster.keeper_reason}</span>
          </div>
          <p className="mt-1 text-caption text-ink-mute">
            The document this group would keep is not in this list — it is grouped under a
            different cluster. Find it before staging these copies, or you may hide every
            copy of the page.
          </p>
        </div>
      )}

      {cluster.members.length < cluster.size ? (
        <p className="text-caption text-gold">
          Showing {formatCount(cluster.members.length)} of {formatCount(cluster.size)}{' '}
          documents in this group. The rest are grouped elsewhere.
        </p>
      ) : null}

      <div>
        <div className="text-label text-ink-mute">
          {formatCount(copies.length)} cop{copies.length === 1 ? 'y' : 'ies'} beside the keeper
        </div>
        {/* Which copies are actionable is not obvious from looking at them, and
            a button that quietly stages a subset is worse than one that says
            so up front. */}
        {unflagged > 0 ? (
          <p className="mt-1 text-caption text-gold">
            {formatCount(unflagged)} of these {unflagged === 1 ? 'is' : 'are'} not flagged —
            dismissed, or not covered by a scan or committed policy yet — so staging leaves{' '}
            {unflagged === 1 ? 'it' : 'them'} alone.
          </p>
        ) : null}
        <ul className="mt-2 space-y-2">
          {copies.map((member) => (
            <li key={member.document_id} className="rounded-md border border-line p-3">
              <div className="break-all text-ink">{member.link ?? member.document_id}</div>
              <MemberMeta member={member} keeper={keeper} />
              {member.candidate_id === null ? (
                <div className="mt-1 text-caption text-gold">not flagged — will not be staged</div>
              ) : null}
            </li>
          ))}
        </ul>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-line pt-3">
        <Button onClick={onStage} disabled={staging || stageable === 0}>
          {staging
            ? 'Staging…'
            : stageable === 0
              ? 'Nothing to stage here'
              : keeper
                ? `Stage ${formatCount(stageable)} and keep 1`
                : `Stage ${formatCount(stageable)}`}
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
        {member.chunk_count ?? '—'} {member.chunk_count === 1 ? 'chunk' : 'chunks'}
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
