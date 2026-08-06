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
import { useMutation } from '@tanstack/react-query';
import { api } from '@/api/client';
import { pruneOverviewQuery } from '@/api/queries';
import { usePruneCommitPolicy } from '@/api/mutations';
import type {
  PruneBundle,
  PruneOverviewResponse,
  PruneSimulateResponse,
} from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Skeleton } from '@/components/primitives/Skeleton';
import { NarrateButton } from '@/components/prune/NarrateButton';
import { NarrationNote } from '@/components/prune/NarrationNote';
import { count as formatCount } from '@/lib/format';

type Tier = 'conservative' | 'standard' | 'aggressive';

/**
 * Preset copy describes the *consequence*, not the mechanism, and says plainly
 * where each level starts producing false positives. A dial that promises only
 * upside is a dial nobody can calibrate against.
 */
const TIERS: { id: Tier; name: string; blurb: string; caution?: string }[] = [
  {
    id: 'conservative',
    name: 'Conservative',
    blurb:
      'Only what is provably redundant: byte-identical copies and documents that produced no text at all.',
    caution: undefined,
  },
  {
    id: 'standard',
    name: 'Standard',
    blurb:
      'Adds verified near-duplicates, same-page-different-URL copies, and files indexed as pages.',
    caution: 'A review band appears — sample it before staging in bulk.',
  },
  {
    id: 'aggressive',
    name: 'Aggressive',
    blurb:
      'Adds semantic duplicates, paraphrase-level matches and off-topic outliers.',
    caution:
      'Expect false positives. Reference pages full of code and tables look like junk to text heuristics — check the sample.',
  },
];

export function TriageTab({ onOpenBundle }: { onOpenBundle: (bundle: PruneBundle) => void }) {
  const overview = useQuery(pruneOverviewQuery);
  const [tier, setTier] = useState<Tier>('standard');

  const simulate = useMutation({
    mutationFn: (body: { tier: Tier; sample: number }) =>
      api.post<PruneSimulateResponse>('/prune/simulate', body),
  });

  // Simulation is explicit rather than automatic on tier change: it is a
  // full-corpus aggregate, and firing it on every click would make the
  // presets feel like they were doing something dangerous.
  const runSimulation = (next: Tier) => {
    setTier(next);
    simulate.mutate({ tier: next, sample: 8 });
  };

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
              <BundleCard key={bundle.key} bundle={bundle} onOpen={() => onOpenBundle(bundle)} />
            ))}
          </div>
        )}
      </section>

      <PolicyDial
        tier={tier}
        onTier={runSimulation}
        result={simulate.data}
        pending={simulate.isPending}
        error={simulate.error?.message}
      />

      {data.by_connector.length > 0 ? <ConnectorTable rows={data.by_connector} /> : null}
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
      <div className="grid gap-4 sm:grid-cols-4">
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

function BundleCard({ bundle, onOpen }: { bundle: PruneBundle; onOpen: () => void }) {
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
        <Button size="sm" variant="secondary" onClick={onOpen}>
          Review
        </Button>
      </div>
    </Card>
  );
}

function PolicyDial({
  tier,
  onTier,
  result,
  pending,
  error,
}: {
  tier: Tier;
  onTier: (tier: Tier) => void;
  result?: PruneSimulateResponse;
  pending: boolean;
  error?: string;
}) {
  const commit = usePruneCommitPolicy();
  const [typed, setTyped] = useState('');

  // Only the band that was actually simulated may be committed, and only
  // against the count that simulation returned — the server rechecks it.
  const autoCount = result?.auto ?? 0;
  const canCommit = !!result && autoCount > 0 && Number(typed) === autoCount;

  return (
    <section className="space-y-3">
      <div>
        <h2 className="font-display font-display-soft text-title text-ink">How aggressive should pruning be?</h2>
        <p className="mt-1 text-label text-ink-mute">
          Pick a level, see exactly what it would flag, then look at a random sample before
          committing. Simulating changes nothing.
        </p>
      </div>

      <div className="grid gap-3 sm:grid-cols-3">
        {TIERS.map((option) => (
          <button
            key={option.id}
            type="button"
            onClick={() => onTier(option.id)}
            aria-pressed={tier === option.id}
            className={`rounded-lg border p-3 text-left transition ${
              tier === option.id
                ? 'border-gold bg-gold/10'
                : 'border-line hover:border-line-2'
            }`}
          >
            <div className="font-display text-ink">{option.name}</div>
            <p className="mt-1 text-caption text-ink-mute">{option.blurb}</p>
            {option.caution ? (
              <p className="mt-1 text-caption text-gold">{option.caution}</p>
            ) : null}
          </button>
        ))}
      </div>

      {pending ? <Skeleton className="h-32 w-full" /> : null}
      {error ? <p className="text-label text-rose">{error}</p> : null}

      {result ? (
        <Card className="space-y-4 p-4">
          <div className="grid gap-4 sm:grid-cols-3">
            <Figure
              label="Would stage in bulk"
              value={formatCount(result.auto)}
              hint="strong enough to act on after a sampled check"
            />
            <Figure
              label="Would need review"
              value={formatCount(result.review)}
              hint="surfaced for a human decision"
            />
            <Figure
              label="Left alone"
              value={formatCount(result.untouched)}
              hint={`of ${formatCount(result.profiled)} measured`}
            />
          </div>

          {result.caveats.length > 0 ? (
            <ul className="space-y-1 rounded-md border border-gold/30 bg-gold/10 p-3">
              {result.caveats.map((caveat) => (
                <li key={caveat} className="text-caption text-gold">
                  {caveat}
                </li>
              ))}
            </ul>
          ) : null}

          {result.by_signal.length > 0 ? (
            <div>
              <div className="text-label text-ink-mute">What is contributing</div>
              <div className="mt-1 flex flex-wrap gap-1.5">
                {result.by_signal.map((signal) => (
                  <Badge key={`${signal.signal}-${signal.band}`} tone="neutral">
                    {signal.signal.replace(/_/g, ' ')} · {formatCount(signal.count)} ({signal.band})
                  </Badge>
                ))}
              </div>
            </div>
          ) : null}

          {result.auto_sample.length > 0 ? (
            <div>
              <div className="text-label text-ink-mute">
                A random sample of what would be staged
              </div>
              <ul className="mt-1 space-y-1">
                {result.auto_sample.map((doc) => (
                  <li key={doc.document_id} className="text-caption">
                    <span className="text-ink">{doc.semantic_id ?? doc.document_id}</span>
                    <span className="text-ink-mute"> — {doc.signals.join(', ')}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {/* Offering "Create 0 candidates" reads as a broken button rather
              than as an empty result, so the empty case says what happened. */}
          {autoCount > 0 ? (
            <div className="flex flex-wrap items-end gap-2 border-t border-line pt-3">
              <label className="text-label text-ink-mute">
                Type {formatCount(autoCount)} to create review rows for the bulk band
                <Input
                  value={typed}
                  onChange={(e) => setTyped(e.target.value)}
                  inputMode="numeric"
                  className="mt-1 w-32"
                  aria-label={`Type ${autoCount} to confirm`}
                />
              </label>
              <Button
                disabled={!canCommit || commit.isPending}
                onClick={() => {
                  commit.mutate(
                    { tier, band: 'auto', confirm_count: autoCount },
                    { onSuccess: () => setTyped('') },
                  );
                }}
              >
                {commit.isPending ? 'Creating…' : `Create ${formatCount(autoCount)} candidates`}
              </Button>
              <p className="text-caption text-ink-mute">
                Creates review rows only. Nothing is hidden or deleted until you stage it, and
                deletion still waits out the grace period.
              </p>
            </div>
          ) : (
            <p className="border-t border-line pt-3 text-caption text-ink-mute">
              This level would not stage anything new
              {result.review > 0
                ? ` — the ${formatCount(result.review)} in the review band need a human decision.`
                : '. Everything it can see is either already under review or left alone.'}
            </p>
          )}
        </Card>
      ) : null}
    </section>
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
