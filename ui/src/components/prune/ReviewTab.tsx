/**
 * Review: the scan launcher (the only button here is a dry scan — it cannot
 * mutate), then the candidate list with filters, evidence chips and the
 * bulk stage/dismiss actions behind their confirmation.
 */
import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { SearchCheck } from 'lucide-react';
import { connectorsQuery, pruneCandidatesQuery, pruneStatusQuery } from '@/api/queries';
import {
  usePruneDismiss,
  usePruneScanCancel,
  usePruneScanCreate,
  usePruneStage,
} from '@/api/mutations';
import type {
  PruneCandidateFilterBody,
  PruneCandidateItem,
  PruneScanDetector,
  PruneScanItem,
  PruneScope,
  PruneStatusResponse,
} from '@/api/types';
import type { QueryParams } from '@/api/client';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Checkbox } from '@/components/primitives/Checkbox';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';
import { CandidateSheet } from './CandidateSheet';
import { PruneConfirmDialog } from './PruneConfirmDialog';
import { chunkLabel, documentLabel, ReasonChips, RiskBadge } from './pruneShared';

const DETECTORS: Array<{ name: PruneScanDetector; label: string; note: string }> = [
  { name: 'exact_duplicate', label: 'Exact duplicates', note: 'identical content hashes; pure database scan' },
  { name: 'thin', label: 'Thin content', note: '0-chunk stubs, age-gated 7 days' },
  { name: 'near_duplicate', label: 'Near duplicates', note: 'MinHash over chunk text — slower, reads the index' },
  { name: 'language', label: 'Language', note: 'reads chunk text; also needs language.enabled in the detector config' },
  { name: 'url_rule', label: 'URL rules', note: 'your enabled URL patterns' },
  { name: 'tag_rule', label: 'Tag rules', note: 'your enabled tag patterns' },
  { name: 'stale', label: 'Stale', note: 'old pages on still-active connectors; report-only shape' },
];

const REASON_DETECTOR_OPTIONS = [
  { value: '', label: 'any detector' },
  { value: 'duplicate', label: 'duplicate' },
  { value: 'thin', label: 'thin' },
  { value: 'language', label: 'language' },
  { value: 'url_rule', label: 'url rule' },
  { value: 'tag_rule', label: 'tag rule' },
  { value: 'stale', label: 'stale' },
  { value: 'recrawl', label: 'recrawled' },
];

const SORT_OPTIONS = [
  { value: 'confidence_desc', label: 'confidence ↓' },
  { value: 'chunks_desc', label: 'chunks ↓' },
  { value: 'chunks_asc', label: 'chunks ↑' },
  { value: 'created_desc', label: 'newest first' },
  { value: 'created_asc', label: 'oldest first' },
];

const PAGE_SIZE = 50;

export function ReviewTab() {
  const status = useQuery(pruneStatusQuery);

  const [detector, setDetector] = useState('');
  const [minConfidence, setMinConfidence] = useState(0);
  const [riskyOnly, setRiskyOnly] = useState(false);
  const [connectorId, setConnectorId] = useState('');
  const [sort, setSort] = useState('confidence_desc');
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [openCandidate, setOpenCandidate] = useState<number | null>(null);
  const [confirming, setConfirming] = useState<'stage-selected' | 'stage-filtered' | null>(null);
  const [dismissForever, setDismissForever] = useState(false);

  const params: QueryParams = useMemo(
    () => ({
      state: 'candidate',
      detector: detector || undefined,
      min_confidence: minConfidence > 0 ? minConfidence : undefined,
      recrawl_risk: riskyOnly ? true : undefined,
      connector_id: connectorId ? Number(connectorId) : undefined,
      sort,
      limit: PAGE_SIZE,
      page,
    }),
    [detector, minConfidence, riskyOnly, connectorId, sort, page],
  );
  const candidates = useQuery(pruneCandidatesQuery(params));

  const filterBody: PruneCandidateFilterBody = {
    state: 'candidate',
    detector: detector || undefined,
    min_confidence: minConfidence > 0 ? minConfidence : undefined,
    recrawl_risk: riskyOnly ? true : undefined,
    connector_id: connectorId ? Number(connectorId) : undefined,
  };

  const items = candidates.data?.items ?? [];
  const selectedItems = items.filter((item) => selected.has(item.id));
  const stage = usePruneStage();
  const dismiss = usePruneDismiss();
  const connectorOptions = useConnectorOptions();

  // The risk breakdown for a *filtered* bulk action is a claim about the whole
  // selection, so it has to come from the server — counting the loaded page
  // would report "none at risk" for a set that holds thousands.
  const riskyTotal = useQuery({
    ...pruneCandidatesQuery({ ...params, recrawl_risk: true, limit: 1, page: 1 }),
    enabled: confirming === 'stage-filtered',
    select: (data) => data.total,
  });

  const clearSelection = () => setSelected(new Set());
  const limits = status.data?.limits;
  const filtersActive =
    detector !== '' || connectorId !== '' || riskyOnly || minConfidence > 0;

  return (
    <div className="space-y-4">
      <ScanLauncher status={status.data} />

      <Card className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={detector}
            onValueChange={(value) => {
              setDetector(value);
              setPage(1);
              clearSelection();
            }}
            options={REASON_DETECTOR_OPTIONS}
            ariaLabel="Filter by detector"
          />
          <Select
            value={connectorId}
            onValueChange={(value) => {
              setConnectorId(value);
              setPage(1);
              clearSelection();
            }}
            options={connectorOptions}
            ariaLabel="Filter by connector"
          />
          <Select value={sort} onValueChange={setSort} options={SORT_OPTIONS} ariaLabel="Sort" />
          <label className="flex items-center gap-2 text-label text-ink-mute">
            <Checkbox checked={riskyOnly} onCheckedChange={(v) => { setRiskyOnly(v); setPage(1); }} label="Only recrawl risk" />
            recrawl risk only
          </label>
          <label className="ml-auto flex items-center gap-2 text-label text-ink-mute">
            min confidence
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={minConfidence}
              aria-label="Minimum confidence"
              onChange={(event) => {
                setMinConfidence(Number(event.target.value));
                setPage(1);
                clearSelection();
              }}
              className="accent-[var(--color-gold)]"
            />
            <span className="w-10 font-mono text-caption text-ink">{minConfidence.toFixed(2)}</span>
          </label>
        </div>

        {candidates.isError ? (
          <ErrorState error={candidates.error} onRetry={() => void candidates.refetch()} />
        ) : candidates.isPending ? (
          <div className="space-y-2">
            <Skeleton className="h-10" />
            <Skeleton className="h-10" />
            <Skeleton className="h-10" />
          </div>
        ) : items.length === 0 ? (
          <EmptyState
            icon={<SearchCheck aria-hidden />}
            title={filtersActive ? 'No candidates match these filters' : 'No candidates to review'}
            description={
              filtersActive
                ? 'Widen the filters, or scan a different scope.'
                : 'Run a scan to find some. A scan is a preview — nothing is hidden or deleted.'
            }
          />
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2 text-label text-ink-mute">
              <Checkbox
                checked={selectedItems.length === items.length && items.length > 0}
                onCheckedChange={(checked) =>
                  setSelected(checked ? new Set(items.map((i) => i.id)) : new Set())
                }
                label="Select every row on this page"
              />
              <span>
                {selectedItems.length > 0
                  ? `${selectedItems.length} selected`
                  : `${formatCount(candidates.data.total)} candidates`}
              </span>
              {selectedItems.length > 0 ? (
                <span className="flex items-center gap-2">
                  <Button
                    size="sm"
                    variant="primary"
                    disabled={!limits}
                    onClick={() => setConfirming('stage-selected')}
                  >
                    Stage selected
                  </Button>
                  <label className="flex items-center gap-1.5">
                    <Checkbox checked={dismissForever} onCheckedChange={setDismissForever} label="Never flag dismissed documents again" />
                    forever
                  </label>
                  <Button
                    size="sm"
                    disabled={dismiss.isPending}
                    onClick={() =>
                      dismiss.mutate(
                        { ids: selectedItems.map((i) => i.id), exclude_future: dismissForever },
                        { onSuccess: clearSelection },
                      )
                    }
                  >
                    Dismiss selected
                  </Button>
                </span>
              ) : candidates.data.total > items.length ? (
                <Button
                  size="sm"
                  className="ml-auto"
                  disabled={!limits}
                  onClick={() => setConfirming('stage-filtered')}
                >
                  Stage all {formatCount(candidates.data.total)} matching…
                </Button>
              ) : null}
            </div>

            <ul className="divide-y divide-line">
              {items.map((item) => (
                <CandidateRow
                  key={item.id}
                  item={item}
                  checked={selected.has(item.id)}
                  onCheck={(checked) =>
                    setSelected((prev) => {
                      const next = new Set(prev);
                      if (checked) next.add(item.id);
                      else next.delete(item.id);
                      return next;
                    })
                  }
                  onOpen={() => setOpenCandidate(item.id)}
                />
              ))}
            </ul>

            <Pager
              page={page}
              hasMore={candidates.data.has_more}
              total={candidates.data.total}
              onPage={(next) => {
                setPage(next);
                clearSelection();
              }}
            />
          </>
        )}
      </Card>

      <CandidateSheet
        candidateId={openCandidate}
        onOpenChange={(open) => {
          if (!open) setOpenCandidate(null);
        }}
      />

      {limits && candidates.data ? (
        <>
          <PruneConfirmDialog
            open={confirming === 'stage-selected'}
            onOpenChange={(open) => setConfirming(open ? 'stage-selected' : null)}
            verb="Stage"
            total={selectedItems.length}
            chunkSum={sumChunks(selectedItems)}
            chunkSumComplete
            riskyCount={selectedItems.filter((i) => i.recrawl_risk).length}
            // Explicit ids: every selected row is in hand, so both numbers
            // above cover the whole selection.
            bigBatch={limits.big_batch}
            graceDays={limits.grace_days}
            consequence="Staged documents are hidden from Onyx search but fully intact; they delete automatically when their grace ends. Restore lives in the Staged tab."
            confirmLabel="Stage — hide from search"
            pending={stage.isPending}
            onConfirm={() =>
              stage.mutate(
                { ids: selectedItems.map((i) => i.id), confirm_count: selectedItems.length },
                {
                  onSuccess: () => {
                    setConfirming(null);
                    clearSelection();
                  },
                },
              )
            }
          />
          <PruneConfirmDialog
            open={confirming === 'stage-filtered'}
            onOpenChange={(open) => setConfirming(open ? 'stage-filtered' : null)}
            verb="Stage"
            total={candidates.data.total}
            chunkSum={sumChunks(items)}
            chunkSumComplete={items.length >= candidates.data.total}
            riskyCount={riskyTotal.data ?? null}
            bigBatch={limits.big_batch}
            graceDays={limits.grace_days}
            consequence="This stages every candidate matching the current filters — the server re-checks the count and refuses if the set changed. Staged documents are hidden from search, data intact, restorable until their grace ends."
            confirmLabel="Stage all matching"
            pending={stage.isPending}
            onConfirm={() =>
              stage.mutate(
                { filter: filterBody, confirm_count: candidates.data.total },
                {
                  onSuccess: () => {
                    setConfirming(null);
                    clearSelection();
                  },
                },
              )
            }
          />
        </>
      ) : null}
    </div>
  );
}

function sumChunks(items: PruneCandidateItem[]): number {
  return items.reduce((sum, item) => sum + (item.chunk_count ?? 0), 0);
}

function useConnectorOptions() {
  const connectors = useQuery(connectorsQuery);
  return useMemo(
    () => [
      { value: '', label: 'any connector' },
      ...(connectors.data ?? [])
        .slice()
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((c) => ({ value: String(c.connector_id), label: c.name })),
    ],
    [connectors.data],
  );
}

function CandidateRow({
  item,
  checked,
  onCheck,
  onOpen,
}: {
  item: PruneCandidateItem;
  checked: boolean;
  onCheck: (checked: boolean) => void;
  onOpen: () => void;
}) {
  return (
    <li className="flex min-h-11 items-center gap-3 px-1 py-2 transition-colors hover:bg-hover">
      <Checkbox checked={checked} onCheckedChange={onCheck} label={`Select ${item.document_id}`} />
      <button
        type="button"
        onClick={onOpen}
        className="min-w-0 flex-1 text-left outline-none focus-visible:rounded focus-visible:ring-1 focus-visible:ring-gold/50"
      >
        <p className="truncate text-label text-ink">{documentLabel(item)}</p>
        <span className="mt-0.5 flex flex-wrap items-center gap-1.5 text-caption text-ink-mute">
          <ReasonChips reasons={item.reasons} />
          {item.connector_name ? <span>{item.connector_name}</span> : null}
          <span>{chunkLabel(item.chunk_count)} chunks</span>
          <RiskBadge item={item} />
        </span>
      </button>
      <ConfidenceCell confidence={item.confidence} />
    </li>
  );
}

/** The number, always — plus a small bar for scanning down the column. */
function ConfidenceCell({ confidence }: { confidence: number }) {
  return (
    <div className="flex w-16 shrink-0 flex-col items-end gap-1">
      <span className="font-mono text-caption text-ink">{confidence.toFixed(2)}</span>
      <div className="h-1 w-full overflow-hidden rounded-full bg-well">
        <div className="h-full rounded-full bg-gold" style={{ width: `${Math.round(confidence * 100)}%` }} />
      </div>
    </div>
  );
}

function Pager({
  page,
  hasMore,
  total,
  onPage,
}: {
  page: number;
  hasMore: boolean;
  total: number;
  onPage: (page: number) => void;
}) {
  return (
    <div className="flex items-center justify-between text-label text-ink-mute">
      <span>
        page {page} · {formatCount(total)} total
      </span>
      <span className="flex gap-2">
        <Button size="sm" disabled={page <= 1} onClick={() => onPage(page - 1)}>
          Previous
        </Button>
        <Button size="sm" disabled={!hasMore} onClick={() => onPage(page + 1)}>
          Next
        </Button>
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Scan launcher
// ---------------------------------------------------------------------------

function ScanLauncher({ status }: { status: PruneStatusResponse | undefined }) {
  const [scopeKind, setScopeKind] = useState<'all' | 'connectors' | 'url_prefix'>('all');
  const [connectorIds, setConnectorIds] = useState<ReadonlySet<number>>(new Set());
  const [prefix, setPrefix] = useState('');
  const [chosen, setChosen] = useState<ReadonlySet<PruneScanDetector>>(
    new Set<PruneScanDetector>(['exact_duplicate', 'thin']),
  );
  const connectors = useQuery(connectorsQuery);
  const createScan = usePruneScanCreate();
  const cancelScan = usePruneScanCancel();

  const active = status?.active_scan ?? null;
  if (active) {
    return <ScanProgress scan={active} onCancel={() => cancelScan.mutate(active.id)} cancelling={cancelScan.isPending} />;
  }

  const scope: PruneScope =
    scopeKind === 'connectors'
      ? { kind: 'connectors', connector_ids: [...connectorIds] }
      : scopeKind === 'url_prefix'
        ? { kind: 'url_prefix', url_prefix: prefix }
        : { kind: 'all' };
  const launchable =
    chosen.size > 0 &&
    (scopeKind === 'all' ||
      (scopeKind === 'connectors' && connectorIds.size > 0) ||
      (scopeKind === 'url_prefix' && prefix.trim() !== ''));

  return (
    <Card className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-display font-display-soft text-title text-ink">Scan for candidates</h2>
        <p className="text-label text-ink-mute">A scan is a preview. Nothing is hidden or deleted.</p>
      </div>

      <div className="flex flex-wrap items-start gap-3">
        <Select
          value={scopeKind}
          onValueChange={(value) => setScopeKind(value as typeof scopeKind)}
          options={[
            { value: 'all', label: 'whole corpus' },
            { value: 'connectors', label: 'chosen connectors' },
            { value: 'url_prefix', label: 'URL prefix' },
          ]}
          ariaLabel="Scan scope"
        />
        {scopeKind === 'url_prefix' ? (
          <Input
            value={prefix}
            onChange={(event) => setPrefix(event.target.value)}
            placeholder="https://example.com/section/"
            aria-label="URL prefix"
            className="min-w-64 flex-1"
          />
        ) : null}
        {scopeKind === 'connectors' ? (
          <div className="max-h-40 min-w-64 flex-1 space-y-1 overflow-y-auto rounded-lg border border-line p-2">
            {(connectors.data ?? [])
              .slice()
              .sort((a, b) => a.name.localeCompare(b.name))
              .map((connector) => (
                <label key={connector.connector_id} className="flex items-center gap-2 text-label text-ink">
                  <Checkbox
                    checked={connectorIds.has(connector.connector_id)}
                    onCheckedChange={(checked) =>
                      setConnectorIds((prev) => {
                        const next = new Set(prev);
                        if (checked) next.add(connector.connector_id);
                        else next.delete(connector.connector_id);
                        return next;
                      })
                    }
                    label={`Scan ${connector.name}`}
                  />
                  <span className="truncate">{connector.name}</span>
                  <span className="ml-auto text-caption text-ink-faint">
                    {formatCount(connector.doc_count)} docs · {connector.status}
                  </span>
                </label>
              ))}
          </div>
        ) : null}
      </div>

      <fieldset className="grid gap-1.5 md:grid-cols-2">
        <legend className="mb-1 text-label font-medium text-ink">Detectors — nothing runs unasked</legend>
        {DETECTORS.map((detector) => (
          <label key={detector.name} className="flex items-start gap-2 text-label text-ink">
            <Checkbox
              checked={chosen.has(detector.name)}
              onCheckedChange={(checked) =>
                setChosen((prev) => {
                  const next = new Set(prev);
                  if (checked) next.add(detector.name);
                  else next.delete(detector.name);
                  return next;
                })
              }
              label={detector.label}
              className="mt-0.5"
            />
            <span>
              {detector.label}
              <span className="block text-caption text-ink-faint">{detector.note}</span>
            </span>
          </label>
        ))}
      </fieldset>

      <div className="flex justify-end">
        <Button
          variant="primary"
          disabled={!launchable || createScan.isPending}
          onClick={() => createScan.mutate({ scope, detectors: [...chosen] })}
        >
          Dry scan
        </Button>
      </div>
    </Card>
  );
}

function ScanProgress({
  scan,
  onCancel,
  cancelling,
}: {
  scan: PruneScanItem;
  onCancel: () => void;
  cancelling: boolean;
}) {
  const percent =
    scan.total && scan.total > 0 ? Math.min(100, Math.round((scan.examined / scan.total) * 100)) : null;
  return (
    <Card className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-display font-display-soft text-title text-ink">
          Scan {scan.id} {scan.status === 'queued' ? 'queued' : 'running'}
        </h2>
        <Button size="sm" onClick={onCancel} disabled={cancelling}>
          Cancel
        </Button>
      </div>
      <p className="text-label text-ink-mute">
        {formatCount(scan.examined)}
        {scan.total ? ` of ${formatCount(scan.total)}` : ''} documents examined ·{' '}
        {scan.detectors.join(', ')} · resumable — a restart continues from the checkpoint
      </p>
      {percent !== null ? (
        <div
          role="progressbar"
          aria-label="Scan progress"
          aria-valuenow={percent}
          aria-valuemin={0}
          aria-valuemax={100}
          className="h-1.5 overflow-hidden rounded-full bg-well"
        >
          <div className="h-full rounded-full bg-gold transition-[width]" style={{ width: `${percent}%` }} />
        </div>
      ) : null}
      <p className="text-caption text-ink-faint">
        found so far: {formatCount(scan.stats.candidates_new ?? 0)} new ·{' '}
        {formatCount(scan.stats.candidates_updated ?? 0)} updated ·{' '}
        {formatCount(scan.stats.excluded_skipped ?? 0)} skipped (excluded)
      </p>
    </Card>
  );
}
