/**
 * Triage — the funnel that replaces the list as the way in.
 *
 * The review list is fine for a few hundred candidates and useless for two
 * hundred thousand. This tab works the other way round: it shows what the
 * backlog is *made of*, lets a threshold be moved against live counts, and
 * only then hands off to a filtered list or a cluster screen.
 *
 * Two rules hold throughout. Every number comes from the server (simulation is
 * a real query against stored measurements, not an estimate), and nothing here
 * hides or deletes anything — committing a policy creates review rows, and the
 * staged → grace → reaper path is unchanged.
 */
import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { pruneOverviewQuery } from '@/api/queries';
import type { PruneBundle, PruneOverviewResponse } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { NarrateButton } from '@/components/prune/NarrateButton';
import { NarrationNote } from '@/components/prune/NarrationNote';
import { PolicyStudio } from '@/components/prune/PolicyStudio';
import { SampleSheet } from '@/components/prune/SampleSheet';
import { count as formatCount } from '@/lib/format';

export function TriageTab({ onOpenBundle }: { onOpenBundle: (bundle: PruneBundle) => void }) {
  const overview = useQuery(pruneOverviewQuery);
  const [sampling, setSampling] = useState<PruneBundle | null>(null);

  if (overview.isPending) {
    return <Skeleton className="h-64 w-full" />;
  }
  if (overview.isError || !overview.data) {
    return (
      <EmptyState
        title="Triage unavailable"
        description={overview.error?.message ?? 'The overview could not be loaded.'}
      />
    );
  }

  const data = overview.data;

  return (
    <div className="space-y-6">
      <CorpusSummary data={data} />

      <section className="space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div>
            <h2 className="font-display font-display-soft text-title text-ink">What the backlog is made of</h2>
            <p className="mt-1 text-label text-ink-mute">
              Groups, not rows. Open one to review it as a filtered list, or approve whole
              duplicate clusters from the Clusters tab.
            </p>
          </div>
          <NarrateButton subjectKind="bundle" disabled={data.bundles.length === 0} />
        </div>
        {data.bundles.length === 0 ? (
          <EmptyState
            title="Nothing flagged yet"
            description="Run a scan from the Review tab. A scan is a preview — it never hides or deletes anything."
          />
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {data.bundles.map((bundle) => (
              <BundleCard
                key={bundle.key}
                bundle={bundle}
                onOpen={() => onOpenBundle(bundle)}
                onSample={() => setSampling(bundle)}
              />
            ))}
          </div>
        )}
      </section>

      <PolicyStudio />

      {data.by_connector.length > 0 ? <ConnectorTable rows={data.by_connector} /> : null}

      {sampling ? <SampleSheet bundle={sampling} onClose={() => setSampling(null)} /> : null}
    </div>
  );
}

function CorpusSummary({ data }: { data: PruneOverviewResponse }) {
  const measuredShare =
    data.documents_total > 0 ? (data.profiled / data.documents_total) * 100 : 0;
  // Rounding a real 0.14% to "0%" reads as "nothing measured" when thousands
  // of documents have been.
  const measuredPct =
    measuredShare > 0 && measuredShare < 1 ? '<1' : Math.round(measuredShare).toString();
  return (
    <Card className="p-4">
      <div className="grid gap-4 sm:grid-cols-3 lg:grid-cols-5">
        <Figure label="Documents" value={formatCount(data.documents_total)} />
        <Figure
          label="Measured"
          value={`${formatCount(data.profiled)}`}
          hint={
            data.profiled < data.documents_total
              ? `${measuredPct}% — unmeasured documents are invisible to policy`
              : 'every document has a profile'
          }
        />
        {/* Verified pairs are the evidence behind every similarity threshold.
            Zero of them is why a semantic policy can simulate to nothing, and
            that is much easier to see here than to infer from a caveat. */}
        <Figure
          label="Verified pairs"
          value={formatCount(data.pairs)}
          hint={
            data.pairs === 0
              ? 'no similarity measured yet — run a near_duplicate scan'
              : 'similarity thresholds can move without re-scanning'
          }
        />
        <Figure label="Flagged" value={formatCount(data.candidates_open)} />
        <Figure
          label="In trash"
          value={formatCount(data.trash.items)}
          hint={
            data.trash.expiring_7d > 0
              ? `${formatCount(data.trash.expiring_7d)} expiring within 7 days`
              : data.trash.items > 0
                ? 'recoverable until their retention ends'
                : undefined
          }
        />
      </div>
    </Card>
  );
}

function Figure({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div>
      <div className="text-label text-ink-mute">{label}</div>
      <div className="font-display text-title text-ink tabular-nums">{value}</div>
      {hint ? <div className="mt-0.5 text-caption text-ink-mute">{hint}</div> : null}
    </div>
  );
}

function BundleCard({
  bundle,
  onOpen,
  onSample,
}: {
  bundle: PruneBundle;
  onOpen: () => void;
  onSample: () => void;
}) {
  return (
    <Card className="flex flex-col gap-2 p-4">
      <div className="flex items-start justify-between gap-2">
        <h3 className="font-display text-ink">{bundle.title}</h3>
        <Badge tone={bundle.mean_confidence >= 0.95 ? 'mint' : 'neutral'}>
          {Math.round(bundle.mean_confidence * 100)}%
        </Badge>
      </div>
      <p className="text-caption text-ink-mute">{bundle.description}</p>
      {/* Additive, never a replacement: the detector's own description above is
          what was actually measured, and it stays whether or not a model has
          had anything to say about the group. */}
      {bundle.narration ? <NarrationNote narration={bundle.narration} /> : null}
      <div className="mt-auto flex items-end justify-between gap-2 pt-2">
        <div>
          <div className="font-display text-title text-ink tabular-nums">
            {formatCount(bundle.documents)}
          </div>
          <div className="text-caption text-ink-mute">
            {formatCount(bundle.chunks)} chunks
            {bundle.recrawl_risk > 0 ? ` · ${formatCount(bundle.recrawl_risk)} may return` : ''}
          </div>
        </div>
        <div className="flex gap-1.5">
          {/* Reviewing a six-figure group one row at a time is the thing this
              screen exists to avoid; a drawn sample is how you decide about the
              whole group without reading all of it. */}
          <Button size="sm" variant="ghost" onClick={onSample}>
            Sample
          </Button>
          <Button size="sm" variant="secondary" onClick={onOpen}>
            Review
          </Button>
        </div>
      </div>
    </Card>
  );
}

function ConnectorTable({
  rows,
}: {
  rows: PruneOverviewResponse['by_connector'];
}) {
  const top = useMemo(() => rows.slice(0, 12), [rows]);
  return (
    <section className="space-y-2">
      <h2 className="font-display font-display-soft text-title text-ink">Where it is concentrated</h2>
      <Card className="overflow-x-auto">
        <table className="w-full text-label">
          <thead className="text-ink-mute">
            <tr className="border-b border-line">
              <th className="p-2 text-left font-normal">Connector</th>
              <th className="p-2 text-right font-normal">Flagged</th>
              <th className="p-2 text-right font-normal">Chunks</th>
              <th className="p-2 text-right font-normal">Mean confidence</th>
            </tr>
          </thead>
          <tbody>
            {top.map((row) => (
              <tr key={row.connector_id ?? 'none'} className="border-b border-line/50">
                <td className="p-2 text-ink">{row.connector_name ?? '—'}</td>
                <td className="p-2 text-right tabular-nums text-ink">
                  {formatCount(row.documents)}
                </td>
                <td className="p-2 text-right tabular-nums text-ink-mute">
                  {formatCount(row.chunks)}
                </td>
                <td className="p-2 text-right tabular-nums text-ink-mute">
                  {Math.round(row.mean_confidence * 100)}%
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </section>
  );
}
