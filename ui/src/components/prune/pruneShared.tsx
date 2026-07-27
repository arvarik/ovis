/**
 * Shared pruning pieces: reason chips, the grace countdown, risk badging.
 * Copy rules (04 §3): say "delete", confidence is always the number, nothing
 * ever says "safe to delete".
 */
import { useEffect, useState } from 'react';
import type { PruneCandidateItem, PruneReason } from '@/api/types';
import { Badge, type BadgeTone } from '@/components/primitives/Badge';

/** Compact chip text per reason — `dup 94%`, `lang deu 0.98`, `rule: x`. */
export function reasonChipText(reason: PruneReason): string {
  switch (reason.detector) {
    case 'duplicate':
      return `dup ${Math.round(reason.confidence * 100)}%`;
    case 'language': {
      const detected =
        typeof reason.evidence.detected === 'string' ? reason.evidence.detected : '?';
      return `lang ${detected} ${reason.confidence.toFixed(2)}`;
    }
    case 'url_rule':
    case 'tag_rule':
      return `rule: ${reason.code}`;
    case 'thin':
      return reason.code === 'chunkless_stub' ? 'stub' : `thin ${reason.confidence.toFixed(1)}`;
    case 'stale':
      return 'stale';
    case 'recrawl':
      return 'recrawled after prune';
    default:
      return reason.detector;
  }
}

export function reasonTone(reason: PruneReason): BadgeTone {
  switch (reason.detector) {
    case 'duplicate':
      return 'violet';
    case 'language':
      return 'indigo';
    case 'url_rule':
    case 'tag_rule':
      return 'teal';
    case 'recrawl':
      return 'gold';
    default:
      return 'neutral';
  }
}

/**
 * "3d 4h" / "2h 10m" / "under a minute" / "due now" — the staged countdown.
 * Server truth (`stage_expires_at`) rendered live; never a client-side clock
 * of its own making.
 */
export function graceCountdown(expiresAtIso: string, nowMs: number): string {
  const remaining = Date.parse(expiresAtIso) - nowMs;
  if (remaining <= 0) return 'due now';
  const minutes = Math.floor(remaining / 60_000);
  if (minutes < 1) return 'under a minute';
  const days = Math.floor(minutes / (60 * 24));
  const hours = Math.floor((minutes % (60 * 24)) / 60);
  const mins = minutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

/**
 * Whether a bulk action needs the count typed back: past the server's
 * big-batch threshold there is no one-click path (04 §5, and `-y` in the CLI
 * has the same rule).
 */
export function needsTypedCount(selectionSize: number, bigBatch: number): boolean {
  return selectionSize > bigBatch;
}

/** Ticking clock for countdowns. One interval per subscriber, coarse is fine. */
export function useNow(intervalMs = 1_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

export function ReasonChips({ reasons }: { reasons: PruneReason[] }) {
  return (
    <span className="flex flex-wrap items-center gap-1">
      {reasons.map((reason) => (
        <Badge key={`${reason.detector}:${reason.code}`} tone={reasonTone(reason)}>
          {reasonChipText(reason)}
        </Badge>
      ))}
    </span>
  );
}

export function RiskBadge({ item }: { item: Pick<PruneCandidateItem, 'recrawl_risk' | 'connector_name'> }) {
  if (!item.recrawl_risk) return null;
  return (
    <Badge
      tone="gold"
      title={`${item.connector_name ?? 'the owning connector'} is still crawling; a deleted copy will likely be re-crawled`}
    >
      recrawl risk
    </Badge>
  );
}

/** `chunk_count: null` is "not counted yet" — never rendered as 0. */
export function chunkLabel(count: number | null): string {
  return count === null ? '—' : String(count);
}

export function documentLabel(item: PruneCandidateItem): string {
  return item.link ?? item.semantic_id ?? item.document_id;
}
