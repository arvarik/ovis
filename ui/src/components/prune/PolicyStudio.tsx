/**
 * The threshold studio — where a policy is chosen, seen and committed.
 *
 * The backend's whole v2 argument is that a threshold should be a review-time
 * decision: a scan records *measurements*, and a policy turns them into bands
 * whenever anyone asks, so moving a number costs one aggregate query instead of
 * re-scanning 1.7 M documents. A three-button preset picker surfaced almost
 * none of that. This surfaces the model as it actually is — every signal
 * editable, the live distribution behind the ones that are continuous, both
 * bands committable, and the result saveable under a name.
 *
 * Two rules hold throughout, both inherited from the API:
 *
 *  - **Simulating changes nothing**, so it is safe on every click — but it is
 *    a full-corpus aggregate, so it is explicit rather than fired per keystroke.
 *  - **A count you can act on must be a count you were shown.** Editing after
 *    simulating invalidates the result rather than silently re-pointing the
 *    commit button at numbers nobody has seen; the server checks the same thing
 *    and answers 409, and this is that check made visible early.
 */
import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';
import { connectorsQuery, prunePoliciesQuery, pruneHistogramQuery } from '@/api/queries';
import { usePruneCommitPolicy } from '@/api/mutations';
import type {
  PruneBand,
  PrunePolicy,
  PruneSampleDoc,
  PruneSimulateResponse,
  PruneThreshold,
} from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Checkbox } from '@/components/primitives/Checkbox';
import { Input } from '@/components/primitives/Input';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';
import { SignalHistogram } from './SignalHistogram';

type Tier = 'conservative' | 'standard' | 'aggressive';

const TIERS: { id: Tier; name: string; blurb: string; caution?: string }[] = [
  {
    id: 'conservative',
    name: 'Conservative',
    blurb: 'Only what is provably redundant: byte-identical copies and documents with no text.',
  },
  {
    id: 'standard',
    name: 'Standard',
    blurb: 'Adds verified near-duplicates, same-page-different-URL copies, and files indexed as pages.',
    caution: 'A review band appears — sample it before staging in bulk.',
  },
  {
    id: 'aggressive',
    name: 'Aggressive',
    blurb: 'Adds semantic duplicates, paraphrase-level matches and off-topic outliers.',
    caution:
      'Expect false positives. Reference pages full of code and tables look like junk to text heuristics.',
  },
];

const BAND_OPTIONS = [
  { value: 'none', label: 'leave alone' },
  { value: 'review', label: 'send to review' },
  { value: 'auto', label: 'stage in bulk' },
];

/** What each band signal means, in the operator's terms rather than the schema's. */
const SIGNALS: { key: 'exact_duplicate' | 'url_variant' | 'asset' | 'stub'; label: string; note: string }[] = [
  {
    key: 'exact_duplicate',
    label: 'Identical copies',
    note: 'Same content hash as another document. The keeper is excluded automatically.',
  },
  {
    key: 'url_variant',
    label: 'Same page, different URL',
    note: 'Canonical URLs match once tracking parameters, scheme and www are folded.',
  },
  {
    key: 'asset',
    label: 'Files indexed as pages',
    note: 'Image, media and archive URLs whose text is the filename. PDFs are not counted here.',
  },
  { key: 'stub', label: 'Empty documents', note: 'Indexed with zero chunks well after their last crawl.' },
];

export function PolicyStudio() {
  const saved = useQuery(prunePoliciesQuery);
  const [policy, setPolicy] = useState<PrunePolicy | null>(null);
  const [tier, setTier] = useState<Tier | null>('standard');
  /** Exactly what produced the numbers on screen; commit sends this, not the editor. */
  const [committed, setCommitted] = useState<{ tier: Tier | null; policy: PrunePolicy } | null>(null);

  const simulate = useMutation({
    mutationFn: (body: { tier?: Tier; policy?: PrunePolicy; sample: number }) =>
      api.post<PruneSimulateResponse>('/prune/simulate', body),
    onSuccess: (result, variables) => {
      // The response carries the resolved policy, so a preset populates the
      // editor from the server's own definition rather than from a copy of the
      // presets kept here — which would drift the first time one changed.
      setPolicy(result.policy);
      setCommitted({ tier: variables.tier ?? null, policy: result.policy });
    },
  });

  // One simulation on arrival, so the screen opens with real numbers instead of
  // an empty frame nobody knows how to fill.
  useEffect(() => {
    simulate.mutate({ tier: 'standard', sample: 6 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const dirty = useMemo(() => {
    if (!policy || !committed) return false;
    return JSON.stringify(policy) !== JSON.stringify(committed.policy);
  }, [policy, committed]);

  const validation = policy ? validate(policy) : null;

  const run = () => {
    if (!policy) return;
    // A preset that has not been edited is sent as a tier so the result says
    // which preset it is; anything else travels as an explicit policy.
    if (tier && !dirty) simulate.mutate({ tier, sample: 6 });
    else simulate.mutate({ policy, sample: 6 });
  };

  const loadTier = (next: Tier) => {
    setTier(next);
    simulate.mutate({ tier: next, sample: 6 });
  };

  const edit = (update: Partial<PrunePolicy>) => {
    setPolicy((prev) => (prev ? { ...prev, ...update } : prev));
    setTier(null); // it is no longer that preset
  };

  return (
    <section className="space-y-3">
      <div>
        <h2 className="font-display font-display-soft text-title text-ink">
          How aggressive should pruning be?
        </h2>
        <p className="mt-1 text-label text-ink-mute">
          Start from a preset or a saved policy, adjust anything, and see exactly what it would
          flag before committing. Simulating changes nothing.
        </p>
      </div>

      <div className="grid gap-3 sm:grid-cols-3">
        {TIERS.map((option) => (
          <button
            key={option.id}
            type="button"
            onClick={() => loadTier(option.id)}
            aria-pressed={tier === option.id && !dirty}
            className={`rounded-lg border p-3 text-left transition ${
              tier === option.id && !dirty ? 'border-gold bg-gold/10' : 'border-line hover:border-line-2'
            }`}
          >
            <div className="font-display text-ink">{option.name}</div>
            <p className="mt-1 text-caption text-ink-mute">{option.blurb}</p>
            {option.caution ? <p className="mt-1 text-caption text-gold">{option.caution}</p> : null}
          </button>
        ))}
      </div>

      {saved.data && saved.data.items.length > 0 ? (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-label text-ink-mute">Saved policies</span>
          {saved.data.items.map((stored) => (
            <button
              key={stored.id}
              type="button"
              onClick={() => {
                setPolicy(stored.body);
                setTier(null);
                simulate.mutate({ policy: stored.body, sample: 6 });
              }}
              className="rounded-full border border-line px-3 py-1 text-caption text-ink hover:border-line-2"
            >
              {stored.name}
              {stored.active ? <span className="ml-1.5 text-mint">· active</span> : null}
            </button>
          ))}
        </div>
      ) : null}

      {policy ? (
        <PolicyEditor policy={policy} onEdit={edit} />
      ) : (
        <Skeleton className="h-64 w-full" />
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button onClick={run} disabled={!policy || simulate.isPending || !!validation}>
          {simulate.isPending ? 'Simulating…' : 'Simulate'}
        </Button>
        {validation ? (
          <p className="text-label text-rose">{validation}</p>
        ) : dirty ? (
          <p className="text-label text-gold">
            Edited since the last simulation — the numbers below are for the previous settings.
          </p>
        ) : (
          <p className="text-caption text-ink-mute">
            A real aggregate over stored measurements, not an estimate. Nothing is created.
          </p>
        )}
      </div>

      {simulate.error ? (
        <p className="text-label text-rose">{(simulate.error as Error).message}</p>
      ) : null}

      {simulate.data && committed ? (
        <SimulationResult
          result={simulate.data}
          stale={dirty}
          committed={committed}
          onSaved={() => void saved.refetch()}
        />
      ) : null}
    </section>
  );
}

/** Mirrors the server's own validation so an obvious mistake is caught early. */
function validate(policy: PrunePolicy): string | null {
  for (const [name, t] of [
    ['Near-duplicate', policy.near_duplicate],
    ['Semantic', policy.semantic],
  ] as const) {
    for (const level of ['auto', 'review'] as const) {
      const value = t[level];
      if (value !== null && (value < 0 || value > 1)) {
        return `${name} thresholds are similarities and must be between 0 and 1.`;
      }
    }
    if (t.auto !== null && t.review !== null && t.auto < t.review) {
      return `${name}: the bulk threshold must be at least the review one — it is the stronger claim.`;
    }
  }
  if (
    policy.off_topic_percentile !== null &&
    (policy.off_topic_percentile < 0 || policy.off_topic_percentile > 50)
  ) {
    return 'Off-topic is a bottom-percentile cut and must be between 0 and 50.';
  }
  const { auto_min_failures: qa, review_min_failures: qr } = policy.quality;
  if (qa !== null && qr !== null && qa < qr) {
    return 'Text quality: the bulk failure count must be at least the review one.';
  }
  return null;
}

function PolicyEditor({
  policy,
  onEdit,
}: {
  policy: PrunePolicy;
  onEdit: (update: Partial<PrunePolicy>) => void;
}) {
  return (
    <Card className="space-y-5 p-4">
      <Group
        title="Provable redundancy"
        note="Structural facts, not judgements — another copy exists, or there is no text at all."
      >
        <div className="grid gap-3 sm:grid-cols-2">
          {SIGNALS.map((signal) => (
            <label key={signal.key} className="space-y-1">
              <span className="text-label text-ink">{signal.label}</span>
              <Select
                value={policy[signal.key]}
                onValueChange={(value) => onEdit({ [signal.key]: value as PruneBand })}
                ariaLabel={signal.label}
                options={BAND_OPTIONS}
              />
              <span className="block text-caption text-ink-mute">{signal.note}</span>
            </label>
          ))}
        </div>
      </Group>

      <Group
        title="Similarity"
        note="How alike two documents have to be before one of them is redundant. The distribution is what the last scan measured."
      >
        <ThresholdRow
          label="Near-duplicate (MinHash overlap)"
          signal="max_jaccard"
          threshold={policy.near_duplicate}
          onChange={(next) => onEdit({ near_duplicate: next })}
        />
        <ThresholdRow
          label="Semantic (embedding cosine)"
          signal="max_cosine"
          threshold={policy.semantic}
          onChange={(next) => onEdit({ semantic: next })}
        />
      </Group>

      <Group
        title="Text quality"
        note="Published Gopher/FineWeb/C4 heuristics. They identify structurally unusual text, which overlaps with — but is not the same as — worthless text."
      >
        <div className="grid gap-3 sm:grid-cols-3">
          <NumberField
            label="Failures for review"
            value={policy.quality.review_min_failures}
            min={1}
            max={14}
            nullable
            onChange={(value) => onEdit({ quality: { ...policy.quality, review_min_failures: value } })}
          />
          <NumberField
            label="Across at least this many families"
            value={policy.quality.min_families}
            min={1}
            max={5}
            onChange={(value) =>
              onEdit({ quality: { ...policy.quality, min_families: value ?? 1 } })
            }
          />
          <NumberField
            label="Failures for bulk staging"
            value={policy.quality.auto_min_failures}
            min={1}
            max={14}
            nullable
            onChange={(value) => onEdit({ quality: { ...policy.quality, auto_min_failures: value } })}
          />
        </div>
        {policy.quality.auto_min_failures !== null ? (
          <p className="rounded-md border border-gold/30 bg-gold/10 p-2 text-caption text-gold">
            No shipped preset lets text quality stage documents without review. API reference pages,
            syntax diagrams and directory listings trip several of these gates at once because code
            and tables genuinely have the text shape of junk.
          </p>
        ) : (
          <p className="text-caption text-ink-mute">
            Leaving the bulk count empty is the shipped behaviour: quality findings always go to a
            human. Requiring failures across distinct families is what stops one page of code
            counting as three independent observations.
          </p>
        )}
      </Group>

      <Group
        title="Off-topic"
        note="Documents furthest from their own connector's centre of mass. Review-only by construction."
      >
        <div className="sm:max-w-xs">
          <NumberField
            label="Bottom percentile"
            value={policy.off_topic_percentile}
            min={0}
            max={50}
            step={0.5}
            nullable
            onChange={(value) => onEdit({ off_topic_percentile: value })}
          />
        </div>
      </Group>

      <Group title="Safety" note="What the policy refuses to do, whatever the numbers say.">
        <label className="flex items-center gap-2 text-label text-ink">
          <Checkbox
            checked={policy.cross_connector_review_only}
            onCheckedChange={(checked) => onEdit({ cross_connector_review_only: checked })}
            label="Hold cross-connector duplicates for review"
          />
          Hold cross-connector duplicates for review
        </label>
        <p className="text-caption text-ink-mute">
          A page mirrored across two sources is usually popular rather than redundant, so it is kept
          out of the bulk band and surfaced for a human instead. Turning this off lets automation
          stage mirrored copies.
        </p>
        <ExemptConnectors
          value={policy.exempt_connectors}
          onChange={(next) => onEdit({ exempt_connectors: next })}
        />
      </Group>
    </Card>
  );
}

function Group({
  title,
  note,
  children,
}: {
  title: string;
  note: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-2 border-t border-line pt-4 first:border-0 first:pt-0">
      <div>
        <h3 className="font-display text-ink">{title}</h3>
        <p className="text-caption text-ink-mute">{note}</p>
      </div>
      {children}
    </div>
  );
}

function ThresholdRow({
  label,
  signal,
  threshold,
  onChange,
}: {
  label: string;
  signal: 'max_jaccard' | 'max_cosine';
  threshold: PruneThreshold;
  onChange: (next: PruneThreshold) => void;
}) {
  const histogram = useQuery(pruneHistogramQuery(signal, 40));
  return (
    <div className="space-y-2 rounded-md border border-line p-3">
      <div className="text-label text-ink">{label}</div>
      <SignalHistogram
        buckets={histogram.data?.buckets ?? []}
        auto={threshold.auto}
        review={threshold.review}
        label={label}
        pending={histogram.isPending}
      />
      <div className="grid gap-3 sm:grid-cols-2">
        <NumberField
          label="Review above"
          value={threshold.review}
          min={0}
          max={1}
          step={0.01}
          nullable
          onChange={(value) => onChange({ ...threshold, review: value })}
        />
        <NumberField
          label="Stage in bulk above"
          value={threshold.auto}
          min={0}
          max={1}
          step={0.01}
          nullable
          onChange={(value) => onChange({ ...threshold, auto: value })}
        />
      </div>
    </div>
  );
}

/**
 * A number that can legitimately be absent.
 *
 * "Off" and "zero" are different answers here — a review threshold of 0 flags
 * everything measured, and `null` disables the signal — so the empty string
 * maps to `null` rather than being coerced to a number.
 */
function NumberField({
  label,
  value,
  min,
  max,
  step,
  nullable,
  onChange,
}: {
  label: string;
  value: number | null;
  min: number;
  max: number;
  step?: number;
  nullable?: boolean;
  onChange: (value: number | null) => void;
}) {
  return (
    <label className="block space-y-1">
      <span className="text-caption text-ink-mute">{label}</span>
      <Input
        type="number"
        inputMode="decimal"
        value={value === null ? '' : value}
        min={min}
        max={max}
        step={step ?? 1}
        placeholder={nullable ? 'off' : undefined}
        aria-label={label}
        onChange={(event) => {
          const raw = event.target.value;
          if (raw === '') {
            onChange(nullable ? null : min);
            return;
          }
          const parsed = Number(raw);
          if (!Number.isNaN(parsed)) onChange(parsed);
        }}
      />
    </label>
  );
}

function ExemptConnectors({
  value,
  onChange,
}: {
  value: string[];
  onChange: (next: string[]) => void;
}) {
  const connectors = useQuery(connectorsQuery);
  const available = (connectors.data ?? [])
    .map((c) => c.name)
    .filter((name): name is string => !!name && !value.includes(name))
    .sort();

  return (
    <div className="space-y-2">
      <span className="text-label text-ink">Connectors this policy never touches</span>
      <div className="flex flex-wrap items-center gap-2">
        {value.map((name) => (
          <button
            key={name}
            type="button"
            onClick={() => onChange(value.filter((n) => n !== name))}
            className="rounded-full border border-line px-3 py-1 text-caption text-ink hover:border-rose/60 hover:text-rose"
            aria-label={`Stop exempting ${name}`}
          >
            {name} ×
          </button>
        ))}
        {available.length > 0 ? (
          <Select
            value=""
            onValueChange={(name) => name && onChange([...value, name])}
            ariaLabel="Exempt a connector from this policy"
            className="w-56"
            options={[
              { value: '', label: 'exempt a connector…' },
              ...available.map((name) => ({ value: name, label: name })),
            ]}
          />
        ) : null}
      </div>
      {value.length > 0 ? (
        <p className="text-caption text-ink-mute">
          The escape hatch for a source that is legitimately code or tables end to end.
        </p>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

function SimulationResult({
  result,
  stale,
  committed,
  onSaved,
}: {
  result: PruneSimulateResponse;
  stale: boolean;
  committed: { tier: Tier | null; policy: PrunePolicy };
  onSaved: () => void;
}) {
  return (
    <Card className={`space-y-4 p-4 ${stale ? 'opacity-60' : ''}`}>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div className="grid flex-1 gap-4 sm:grid-cols-3">
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
        <span className="font-mono text-caption text-ink-faint" title="Policy fingerprint">
          {result.policy_hash.slice(0, 12)}
        </span>
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
              <Badge
                key={`${signal.signal}-${signal.band}`}
                tone={signal.band === 'auto' ? 'violet' : 'neutral'}
              >
                {signal.signal.replace(/_/g, ' ')} · {formatCount(signal.count)} ({signal.band})
              </Badge>
            ))}
          </div>
        </div>
      ) : null}

      {result.by_connector.length > 0 ? (
        <details className="rounded-md border border-line p-3">
          <summary className="cursor-pointer text-label text-ink-mute">
            Where it lands — {result.by_connector.length} connector
            {result.by_connector.length === 1 ? '' : 's'} affected
          </summary>
          <table className="mt-2 w-full text-label">
            <thead className="text-ink-mute">
              <tr className="border-b border-line">
                <th className="p-1.5 text-left font-normal">Connector</th>
                <th className="p-1.5 text-right font-normal">Bulk</th>
                <th className="p-1.5 text-right font-normal">Review</th>
              </tr>
            </thead>
            <tbody>
              {result.by_connector.map((row) => (
                <tr key={row.connector_id ?? 'none'} className="border-b border-line/50">
                  <td className="p-1.5 text-ink">{row.connector_name ?? '—'}</td>
                  <td className="p-1.5 text-right tabular-nums text-ink">{formatCount(row.auto)}</td>
                  <td className="p-1.5 text-right tabular-nums text-ink-mute">
                    {formatCount(row.review)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </details>
      ) : null}

      <div className="grid gap-3 sm:grid-cols-2">
        <BandSample
          title="A random sample of what would be staged in bulk"
          docs={result.auto_sample}
          empty="Nothing in the bulk band."
        />
        <BandSample
          title="A random sample of what would go to review"
          docs={result.review_sample}
          empty="Nothing in the review band."
        />
      </div>

      {stale ? (
        <p className="border-t border-line pt-3 text-caption text-gold">
          These numbers are for the previous settings. Simulate again before committing.
        </p>
      ) : (
        <CommitBar result={result} committed={committed} onSaved={onSaved} />
      )}
    </Card>
  );
}

function BandSample({
  title,
  docs,
  empty,
}: {
  title: string;
  docs: PruneSampleDoc[];
  empty: string;
}) {
  return (
    <div className="rounded-md border border-line p-3">
      <div className="text-label text-ink-mute">{title}</div>
      {docs.length === 0 ? (
        <p className="mt-1 text-caption text-ink-faint">{empty}</p>
      ) : (
        <ul className="mt-1 space-y-1">
          {docs.map((doc) => (
            <li key={doc.document_id} className="text-caption">
              <span className="text-ink">{doc.semantic_id ?? doc.document_id}</span>
              {doc.signals.length > 0 ? (
                <span className="text-ink-mute"> — {doc.signals.join(', ')}</span>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Turning a band into candidates.
 *
 * Both bands are committable, because they mean different things: the bulk band
 * is what a sampled check can approve wholesale, and the review band is the
 * queue a person works through. Either way this only creates review rows —
 * staging is still a separate confirmed action and deletion still waits out the
 * grace period.
 */
function CommitBar({
  result,
  committed,
  onSaved,
}: {
  result: PruneSimulateResponse;
  committed: { tier: Tier | null; policy: PrunePolicy };
  onSaved: () => void;
}) {
  const commit = usePruneCommitPolicy();
  const [band, setBand] = useState<'auto' | 'review'>('auto');
  const [typed, setTyped] = useState('');
  const [saveAs, setSaveAs] = useState('');

  const expected = band === 'auto' ? result.auto : result.review;
  const canCommit = expected > 0 && Number(typed) === expected;

  if (result.auto === 0 && result.review === 0) {
    return (
      <p className="border-t border-line pt-3 text-caption text-ink-mute">
        This policy would not flag anything new. Everything it can see is either already under
        review, excluded, or left alone.
      </p>
    );
  }

  return (
    <div className="space-y-3 border-t border-line pt-3">
      <div className="flex flex-wrap items-end gap-2">
        <label className="space-y-1">
          <span className="block text-caption text-ink-mute">Create candidates from</span>
          <Select
            value={band}
            onValueChange={(value) => {
              setBand(value as 'auto' | 'review');
              setTyped('');
            }}
            ariaLabel="Which band to turn into candidates"
            className="w-56"
            options={[
              { value: 'auto', label: `bulk band · ${formatCount(result.auto)}` },
              { value: 'review', label: `review band · ${formatCount(result.review)}` },
            ]}
          />
        </label>

        <label className="text-label text-ink-mute">
          Type {formatCount(expected)} to confirm
          <Input
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            inputMode="numeric"
            className="mt-1 w-32"
            aria-label={`Type ${expected} to confirm`}
            disabled={expected === 0}
          />
        </label>

        <label className="text-label text-ink-mute">
          Save this policy as
          <Input
            value={saveAs}
            onChange={(e) => setSaveAs(e.target.value)}
            className="mt-1 w-48"
            placeholder="optional name"
            aria-label="Save this policy under a name"
          />
        </label>

        <Button
          disabled={!canCommit || commit.isPending}
          onClick={() =>
            commit.mutate(
              {
                ...(committed.tier ? { tier: committed.tier } : { policy: committed.policy }),
                band,
                confirm_count: expected,
                save_as: saveAs.trim() || undefined,
              },
              {
                onSuccess: () => {
                  setTyped('');
                  if (saveAs.trim()) {
                    setSaveAs('');
                    onSaved();
                  }
                },
              },
            )
          }
        >
          {commit.isPending
            ? 'Creating…'
            : `Create ${formatCount(expected)} candidate${expected === 1 ? '' : 's'}`}
        </Button>
      </div>

      <p className="text-caption text-ink-mute">
        Creates review rows only. Nothing is hidden or deleted until you stage it, and deletion
        still waits out the grace period.
        {saveAs.trim() ? ' Saving also makes this the active policy.' : ''}
      </p>
    </div>
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
