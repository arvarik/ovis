import { useState } from 'react';
import { Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import {
  Area,
  AreaChart,
  CartesianGrid,
  Cell,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip as ChartTooltip,
  XAxis,
  YAxis,
} from 'recharts';
import {
  healthQuery,
  overviewQuery,
  runtimeQuery,
  sourcesQuery,
  timelineQuery,
  topConnectorsQuery,
} from '@/api/queries';
import { cn } from '@/lib/cn';
import { absolute, bytes, compact, count as formatCount, sourceLabel } from '@/lib/format';
import { Card } from '@/components/primitives/Card';
import { ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';
import { CountUp } from './CountUp';

/**
 * Categorical palette for source identity — token hues in fixed order,
 * validated with the dataviz six-checks (CVD ΔE 17.3, normal 24.7, contrast
 * ≥3:1 on surface). Identity is never color-alone: every slice carries a
 * direct label + count.
 */
const SOURCE_COLORS = [
  'var(--color-mint)',
  'var(--color-indigo)',
  'var(--color-gold)',
  'var(--color-teal)',
];

/** Status palette for attempt outcomes — reserved, labeled, never color-alone. */
const OUTCOME_SPECS = [
  { key: 'success', label: 'success', className: 'bg-mint' },
  { key: 'completed_with_errors', label: 'partial', className: 'bg-gold' },
  { key: 'failed', label: 'failed', className: 'bg-rose' },
  { key: 'canceled', label: 'canceled', className: 'bg-ink-faint' },
] as const;

const WINDOWS = ['24h', '7d', '30d'] as const;

function ChartTooltipContent({
  active,
  payload,
  label,
}: {
  active?: boolean;
  payload?: Array<{ value: number }>;
  label?: string;
}) {
  if (!active || !payload || payload.length === 0) return null;
  return (
    <div className="glass-panel rounded-lg px-3 py-2">
      <p className="font-mono text-caption text-ink-faint">{label}</p>
      <p className="text-body text-ink">{formatCount(payload[0]?.value ?? 0)} docs</p>
    </div>
  );
}

function TimelineCard() {
  const [window, setWindow] = useState<(typeof WINDOWS)[number]>('24h');
  const timeline = useQuery(timelineQuery(window));

  const data = (timeline.data?.items ?? []).map((b) => ({
    label:
      window === '24h'
        ? absolute(b.bucket).slice(11)
        : absolute(b.bucket).slice(5, 10),
    docs: b.docs,
  }));

  return (
    <Card className="md:col-span-2">
      <div className="mb-3 flex items-center justify-between gap-2">
        <h2 className="text-label font-medium text-ink-faint">Documents indexed</h2>
        <div role="group" aria-label="Window" className="flex items-center gap-1">
          {WINDOWS.map((w) => (
            <button
              key={w}
              type="button"
              aria-pressed={window === w}
              onClick={() => setWindow(w)}
              className={cn(
                'min-h-11 rounded-full border px-3 text-caption transition-colors md:min-h-7',
                window === w
                  ? 'border-gold/40 bg-gold/15 text-gold'
                  : 'border-line bg-surface text-ink-mute hover:bg-hover',
              )}
            >
              {w}
            </button>
          ))}
        </div>
      </div>
      {timeline.isPending ? (
        <Skeleton className="h-56 w-full rounded-lg" />
      ) : timeline.isError ? (
        <ErrorState error={timeline.error} title="Timeline unavailable" onRetry={() => void timeline.refetch()} />
      ) : (
        <div className="h-56">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 4 }}>
              <defs>
                <linearGradient id="timeline-fill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--color-mint)" stopOpacity={0.25} />
                  <stop offset="100%" stopColor="var(--color-mint)" stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--color-line)" strokeOpacity={0.4} vertical={false} />
              <XAxis
                dataKey="label"
                tick={{ fill: 'var(--color-ink-faint)', fontSize: 11 }}
                tickLine={false}
                axisLine={{ stroke: 'var(--color-line)' }}
                minTickGap={32}
              />
              <YAxis
                tick={{ fill: 'var(--color-ink-faint)', fontSize: 11 }}
                tickLine={false}
                axisLine={false}
                tickFormatter={(v: number) => compact(v)}
                width={44}
              />
              <ChartTooltip
                content={<ChartTooltipContent />}
                cursor={{ stroke: 'var(--color-gold)', strokeOpacity: 0.5 }}
              />
              <Area
                type="monotone"
                dataKey="docs"
                stroke="var(--color-mint)"
                strokeWidth={2}
                fill="url(#timeline-fill)"
                activeDot={{ r: 4, fill: 'var(--color-gold)', stroke: 'var(--color-surface)', strokeWidth: 2 }}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      )}
    </Card>
  );
}

function SourcesCard() {
  const sources = useQuery(sourcesQuery);
  const items = sources.data ?? [];
  return (
    <Card>
      <h2 className="mb-3 text-label font-medium text-ink-faint">Sources</h2>
      {sources.isPending ? (
        <Skeleton className="h-40 w-full rounded-lg" />
      ) : sources.isError ? (
        <ErrorState error={sources.error} title="Sources unavailable" onRetry={() => void sources.refetch()} />
      ) : (
        <div className="flex items-center gap-4">
          <div className="h-36 w-36 shrink-0">
            <ResponsiveContainer width="100%" height="100%">
              <PieChart>
                <Pie
                  data={items.map((s) => ({ name: s.source, value: Math.max(s.documents, 0) }))}
                  dataKey="value"
                  innerRadius="62%"
                  outerRadius="100%"
                  paddingAngle={2}
                  stroke="var(--color-surface)"
                  strokeWidth={2}
                  isAnimationActive={false}
                >
                  {items.map((s, i) => (
                    <Cell key={s.source} fill={SOURCE_COLORS[i % SOURCE_COLORS.length]} />
                  ))}
                </Pie>
              </PieChart>
            </ResponsiveContainer>
          </div>
          <ul className="min-w-0 flex-1 space-y-1.5">
            {items.map((s, i) => (
              <li key={s.source} className="flex items-center gap-2">
                <span
                  aria-hidden
                  className="size-2.5 shrink-0 rounded-full"
                  style={{ background: SOURCE_COLORS[i % SOURCE_COLORS.length] }}
                />
                <span className="min-w-0 flex-1 truncate text-label text-ink-mute">
                  {sourceLabel(s.source)}
                </span>
                <span className="font-mono text-caption text-ink-faint">
                  {compact(s.documents)} · {s.connectors} conn
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Card>
  );
}

function TopConnectorsCard() {
  const top = useQuery(topConnectorsQuery(10));
  const items = top.data ?? [];
  const max = Math.max(...items.map((c) => c.doc_count), 1);
  return (
    <Card className="md:col-span-2">
      <h2 className="mb-3 text-label font-medium text-ink-faint">Top connectors by documents</h2>
      {top.isPending ? (
        <Skeleton className="h-48 w-full rounded-lg" />
      ) : top.isError ? (
        <ErrorState error={top.error} title="Leaderboard unavailable" onRetry={() => void top.refetch()} />
      ) : (
        <ul className="space-y-1.5">
          {items.map((c) => (
            <li key={c.cc_pair_id}>
              <Link
                to="/connectors/$ccPairId"
                params={{ ccPairId: c.cc_pair_id }}
                className="group grid grid-cols-[minmax(0,11rem)_1fr_5rem] items-center gap-3 rounded-lg px-1.5 py-1 transition-colors hover:bg-hover"
              >
                <span className="truncate text-label text-ink-mute group-hover:text-ink">
                  {c.name}
                </span>
                <span aria-hidden className="h-3 overflow-hidden rounded-sm bg-well">
                  <span
                    className="block h-full rounded-sm bg-mint/80"
                    style={{ width: `${Math.max((c.doc_count / max) * 100, 1)}%` }}
                  />
                </span>
                <span className="text-right font-mono text-mono-sm text-ink-mute">
                  {formatCount(c.doc_count)}
                </span>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

export function StatsView() {
  const overview = useQuery(overviewQuery);
  const runtime = useQuery(runtimeQuery);
  const health = useQuery(healthQuery);

  if (overview.isError) {
    return (
      <ErrorState error={overview.error} title="Stats could not load" onRetry={() => void overview.refetch()} />
    );
  }

  const o = overview.data;
  const attempts = o?.attempts;
  const outcomeTotal = attempts
    ? OUTCOME_SPECS.reduce((sum, s) => sum + attempts[s.key], 0)
    : 0;
  const diskPct = o?.index.disk_used_pct ?? null;

  return (
    <div className="h-full overflow-y-auto overscroll-contain">
      <div className="mx-auto max-w-5xl space-y-3 p-3 pb-24 md:p-4">
        <h1 className="font-display font-display-soft text-display text-ink">Overview</h1>

        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          <Card className="flex flex-col gap-1.5">
            <div className="text-label text-ink-mute">Documents</div>
            <div className="stat-numeral text-display-xl leading-none text-ink">
              {o ? (
                <>
                  {o.documents_exact ? '' : <span className="text-ink-faint">~</span>}
                  <CountUp value={o.documents} />
                </>
              ) : (
                '…'
              )}
            </div>
            {o && !o.documents_exact ? (
              <div className="font-mono text-caption text-ink-faint">planner estimate</div>
            ) : null}
          </Card>

          <Card className="flex flex-col gap-1.5">
            <div className="text-label text-ink-mute">Chunks</div>
            <div className="stat-numeral text-display-xl leading-none text-ink">
              {o ? (
                o.chunks !== null ? (
                  <CountUp value={o.chunks} format={compact} />
                ) : (
                  <span className="text-ink-faint" title="OpenSearch did not answer the count">
                    —
                  </span>
                )
              ) : (
                '…'
              )}
            </div>
            {o ? (
              <div className="font-mono text-caption text-ink-faint">
                {o.embedding.model} · {o.embedding.dim}d
              </div>
            ) : null}
          </Card>

          <Card className="flex flex-col gap-1.5">
            <div className="text-label text-ink-mute">Index size</div>
            <div
              className={cn(
                'stat-numeral text-display-xl leading-none',
                o?.index.read_only ? 'text-rose' : 'text-ink',
              )}
            >
              {o?.index.size_bytes != null ? bytes(o.index.size_bytes) : '…'}
            </div>
            {diskPct !== null ? (
              <div className="space-y-1">
                <div
                  role="meter"
                  aria-valuenow={Math.round(diskPct)}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-label="Disk used"
                  className="h-1.5 overflow-hidden rounded-full bg-well"
                >
                  <div
                    className={cn(
                      'h-full rounded-full',
                      diskPct >= 85 ? 'bg-rose' : diskPct >= 75 ? 'bg-gold' : 'bg-mint',
                    )}
                    style={{ width: `${Math.min(diskPct, 100)}%` }}
                  />
                </div>
                <div
                  className={cn(
                    'font-mono text-caption',
                    o?.index.read_only ? 'font-medium text-rose' : 'text-ink-faint',
                  )}
                >
                  {o?.index.read_only
                    ? 'READ-ONLY — flood-stage watermark tripped'
                    : `${diskPct.toFixed(1)}% disk used`}
                </div>
              </div>
            ) : null}
          </Card>

          <Card className="flex flex-col gap-1.5">
            <div className="text-label text-ink-mute">Connectors</div>
            <div className="stat-numeral text-display-xl leading-none text-ink">
              {o ? <CountUp value={o.connectors.total} /> : '…'}
            </div>
            {o ? (
              <div className="font-mono text-caption text-ink-faint">
                <span className="text-mint">{o.connectors.active} active</span> ·{' '}
                {o.connectors.paused} paused · <span className="text-gold">{o.connectors.parked} parked</span>
              </div>
            ) : null}
          </Card>
        </div>

        <div className="grid gap-3 md:grid-cols-2">
          <TimelineCard />
          <SourcesCard />

          <Card>
            <h2 className="mb-3 text-label font-medium text-ink-faint">Attempt outcomes</h2>
            {attempts && outcomeTotal > 0 ? (
              <div className="space-y-3">
                <div className="flex h-4 gap-0.5 overflow-hidden rounded-full" role="img" aria-label="Attempt outcome shares">
                  {OUTCOME_SPECS.filter((s) => attempts[s.key] > 0).map((s) => (
                    <span
                      key={s.key}
                      className={cn('h-full', s.className)}
                      style={{ width: `${(attempts[s.key] / outcomeTotal) * 100}%` }}
                    />
                  ))}
                </div>
                <ul className="space-y-1">
                  {OUTCOME_SPECS.map((s) => (
                    <li key={s.key} className="flex items-center gap-2">
                      <span aria-hidden className={cn('size-2.5 rounded-full', s.className)} />
                      <span className="flex-1 text-label text-ink-mute">{s.label}</span>
                      <span className="font-mono text-caption text-ink-faint">
                        {formatCount(attempts[s.key])}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <Skeleton className="h-24 w-full rounded-lg" />
            )}
          </Card>

          <TopConnectorsCard />

          <Card className="md:col-span-2">
            <h2 className="mb-2 text-label font-medium text-ink-faint">Runtime</h2>
            <dl className="grid grid-cols-1 gap-x-6 gap-y-1.5 font-mono text-caption text-ink-mute sm:grid-cols-2">
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">index</dt>
                <dd className="truncate">{runtime.data?.index_name ?? '…'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">embedding</dt>
                <dd>
                  {runtime.data ? `${runtime.data.embedding_model} · ${runtime.data.embedding_dim}d` : '…'}
                </dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">search settings</dt>
                <dd>#{runtime.data?.search_settings_id ?? '…'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">query prefix</dt>
                <dd className="truncate">{runtime.data ? `“${runtime.data.query_prefix}”` : '…'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                {/* The Onyx version lives on /system/health, NOT /system/runtime. */}
                <dt className="text-ink-faint">onyx</dt>
                <dd>{health.data?.onyx_api.version ?? '—'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">ovis</dt>
                <dd>{health.data ? `v${health.data.version}` : '…'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">refreshed</dt>
                <dd>{runtime.data ? absolute(runtime.data.refreshed_at) : '…'}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-faint">schema</dt>
                <dd className={runtime.data?.schema_ok === false ? 'text-gold' : ''}>
                  {runtime.data ? (runtime.data.schema_ok ? 'ok' : 'drift detected') : '…'}
                </dd>
              </div>
            </dl>
          </Card>
        </div>
      </div>
    </div>
  );
}
