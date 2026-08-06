/**
 * What past scans found.
 *
 * A scan records a great deal — how much it examined, what each detector hit,
 * how many candidates it opened, updated and closed, which configuration it ran
 * under — and none of it was reachable once the run finished: the UI watched
 * the active scan and then forgot it existed. That made two ordinary questions
 * unanswerable. "Did the threshold change I made actually do anything?" and
 * "where did these 12,000 candidates come from?"
 *
 * So each run keeps its numbers, and the candidates it produced are one click
 * away — the API has always taken `scan_id` as a filter.
 */
import { useQuery } from '@tanstack/react-query';
import { pruneScansQuery } from '@/api/queries';
import type { PruneScanItem, PruneScope } from '@/api/types';
import { Badge, type BadgeTone } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount, relative } from '@/lib/format';

function statusTone(status: PruneScanItem['status']): BadgeTone {
  switch (status) {
    case 'done':
      return 'mint';
    case 'failed':
      return 'rose';
    case 'cancelled':
      return 'neutral';
    default:
      return 'gold';
  }
}

function scopeLabel(scope: PruneScope): string {
  switch (scope.kind) {
    case 'connectors':
      return `${scope.connector_ids?.length ?? 0} connector${scope.connector_ids?.length === 1 ? '' : 's'}`;
    case 'url_prefix':
      return scope.url_prefix ?? 'URL prefix';
    default:
      return 'whole corpus';
  }
}

/**
 * The counters worth a line of text, in the order someone reads them: what it
 * looked at, then what changed as a result. Zeroes are dropped — a scan that
 * closed nothing should not spend a chip saying so.
 */
const HEADLINE: { key: string; label: string; plural?: string; tone?: BadgeTone }[] = [
  { key: 'candidates_new', label: 'new', tone: 'violet' },
  { key: 'candidates_updated', label: 'updated' },
  { key: 'candidates_closed', label: 'closed', tone: 'mint' },
  { key: 'profiles_written', label: 'measured' },
  { key: 'dup_groups', label: 'duplicate group', plural: 'duplicate groups' },
  { key: 'url_variant_groups', label: 'URL group', plural: 'URL groups' },
  { key: 'quality_hits', label: 'quality flag', plural: 'quality flags' },
  { key: 'asset_hits', label: 'asset', plural: 'assets' },
  { key: 'near_pairs_verified', label: 'pair verified', plural: 'pairs verified' },
  { key: 'content_errors', label: 'content error', plural: 'content errors', tone: 'rose' },
];

export function ScanHistory({ onViewCandidates }: { onViewCandidates: (scanId: number) => void }) {
  const scans = useQuery(pruneScansQuery(8));
  const items = (scans.data?.items ?? []).filter((scan) => scan.status !== 'running');

  if (scans.isPending) return <Skeleton className="h-24 w-full" />;
  if (items.length === 0) return null;

  return (
    <Card className="space-y-2">
      <h2 className="font-display font-display-soft text-title text-ink">Recent scans</h2>
      <ul className="divide-y divide-line">
        {items.map((scan) => {
          const stats = scan.stats ?? {};
          const found = HEADLINE.filter((entry) => (stats[entry.key] ?? 0) > 0);
          return (
            <li key={scan.id} className="flex flex-wrap items-start gap-2 py-2.5">
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-center gap-2 text-label text-ink">
                  <Badge tone={statusTone(scan.status)}>{scan.status}</Badge>
                  <span>{scan.detectors.join(', ')}</span>
                  <span className="text-ink-mute">· {scopeLabel(scan.scope)}</span>
                </p>
                <p className="mt-1 flex flex-wrap items-center gap-1.5 text-caption text-ink-mute">
                  <span>
                    {formatCount(scan.examined)} examined
                    {scan.total !== null ? ` of ${formatCount(scan.total)}` : ''}
                  </span>
                  {found.map((entry) => {
                    const n = stats[entry.key] ?? 0;
                    return (
                      <Badge key={entry.key} tone={entry.tone ?? 'neutral'}>
                        {formatCount(n)} {n === 1 ? entry.label : (entry.plural ?? entry.label)}
                      </Badge>
                    );
                  })}
                  {found.length === 0 && scan.status === 'done' ? (
                    <span>nothing flagged — the corpus already matches this configuration</span>
                  ) : null}
                </p>
                {scan.error ? <p className="mt-1 text-caption text-rose">{scan.error}</p> : null}
              </div>
              <span className="text-caption text-ink-faint">
                {relative(scan.finished_at ?? scan.created_at)}
              </span>
              {(stats.candidates_new ?? 0) > 0 || (stats.candidates_updated ?? 0) > 0 ? (
                <Button size="sm" variant="secondary" onClick={() => onViewCandidates(scan.id)}>
                  See its candidates
                </Button>
              ) : null}
            </li>
          );
        })}
      </ul>
    </Card>
  );
}
