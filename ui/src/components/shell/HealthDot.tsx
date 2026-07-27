import { Popover } from 'radix-ui';
import { useQuery } from '@tanstack/react-query';
import { healthQuery, runtimeQuery } from '@/api/queries';
import type { DependencyHealth } from '@/api/types';
import { cn } from '@/lib/cn';

type Level = 'ok' | 'degraded' | 'down';

function level(status: string | undefined, isError: boolean): Level {
  if (isError) return 'down';
  if (status === 'ok') return 'ok';
  if (status === undefined) return 'down';
  return 'degraded';
}

const DOT: Record<Level, string> = {
  ok: 'bg-mint',
  degraded: 'bg-gold animate-pulse-dot',
  down: 'bg-rose animate-pulse-dot',
};

function DepRow({ name, dep }: { name: string; dep: DependencyHealth }) {
  const lv = level(dep.status, false);
  return (
    <div className="flex items-center justify-between gap-4 py-1">
      <span className="flex items-center gap-2 text-label text-ink">
        <span aria-hidden className={cn('size-1.5 rounded-full', DOT[lv])} />
        {name}
      </span>
      <span className="font-mono text-caption text-ink-mute">
        {dep.status}
        {dep.latency_ms !== null ? ` · ${dep.latency_ms.toFixed(1)} ms` : ''}
      </span>
    </div>
  );
}

/**
 * The system status indicator. Silence is the healthy state: nothing renders
 * while everything is ok, and the dot (gold degraded / rose unreachable)
 * appears only when there is something to say. Tap → dependency latencies,
 * index name, versions — live data only.
 */
export function HealthDot() {
  const health = useQuery(healthQuery);
  const runtime = useQuery(runtimeQuery);

  const lv: Level = health.isError
    ? 'down'
    : health.data
      ? health.data.status === 'ok'
        ? 'ok'
        : 'degraded'
      : 'ok';

  if (lv === 'ok') return null;

  const label = lv === 'degraded' ? 'Degraded' : 'Backend unreachable';

  return (
    <Popover.Root>
      <Popover.Trigger
        aria-label={`System status: ${label}`}
        className="flex size-11 md:size-8 items-center justify-center rounded-lg transition-colors hover:bg-hover"
      >
        <span aria-hidden className={cn('size-2.5 rounded-full', DOT[lv])} />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          sideOffset={8}
          collisionPadding={12}
          className="glass-panel z-50 w-80 max-w-[calc(100vw-24px)] rounded-xl p-4 animate-scale-in"
        >
          <div className="mb-2 flex items-baseline justify-between">
            <h3 className="font-display font-display-soft text-title text-ink">System</h3>
            <span
              className={cn(
                'text-caption font-medium',
                lv === 'degraded' ? 'text-gold' : 'text-rose',
              )}
            >
              {label}
            </span>
          </div>

          {health.isError ? (
            <p className="text-label text-ink-mute">
              The OVIS backend did not answer the health probe. Check that the server is running.
            </p>
          ) : health.data ? (
            <>
              <div className="divide-y divide-line/60">
                <DepRow name="Postgres" dep={health.data.postgres} />
                <DepRow name="OpenSearch" dep={health.data.opensearch} />
                <DepRow
                  name="Onyx API"
                  dep={{
                    status: health.data.onyx_api.configured
                      ? health.data.onyx_api.status
                      : 'not configured',
                    latency_ms: health.data.onyx_api.latency_ms,
                    detail: health.data.onyx_api.detail,
                  }}
                />
                <DepRow name="Embedder" dep={health.data.embedder} />
              </div>

              <dl className="mt-3 space-y-1 border-t border-line pt-3 font-mono text-caption text-ink-mute">
                <div className="flex justify-between gap-3">
                  <dt className="text-ink-faint">index</dt>
                  <dd className="truncate">{health.data.index_name}</dd>
                </div>
                {runtime.data ? (
                  <div className="flex justify-between gap-3">
                    <dt className="text-ink-faint">model</dt>
                    <dd className="truncate">
                      {runtime.data.embedding_model} · {runtime.data.embedding_dim}d
                    </dd>
                  </div>
                ) : null}
                <div className="flex justify-between gap-3">
                  <dt className="text-ink-faint">ovis</dt>
                  <dd>v{health.data.version}</dd>
                </div>
                {health.data.onyx_api.version ? (
                  <div className="flex justify-between gap-3">
                    <dt className="text-ink-faint">onyx</dt>
                    <dd>{health.data.onyx_api.version}</dd>
                  </div>
                ) : null}
                {!health.data.schema_ok ? (
                  <div className="text-gold">schema drift detected</div>
                ) : null}
              </dl>
            </>
          ) : (
            <p className="text-label text-ink-mute">Checking…</p>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
