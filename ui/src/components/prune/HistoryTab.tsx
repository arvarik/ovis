/**
 * History: the audit trail, rendered honestly — per-batch delete outcomes
 * include `chunks_deleted` and any `index_cleanup_pending`, never a blanket
 * "deleted".
 */
import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { ScrollText } from 'lucide-react';
import { pruneAuditQuery } from '@/api/queries';
import type { PruneAuditItem } from '@/api/types';
import { Badge, type BadgeTone } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount, relative } from '@/lib/format';
import { useDebouncedValue } from '@/hooks/useDebouncedValue';

const ACTION_OPTIONS = [
  { value: '', label: 'any action' },
  { value: 'staged', label: 'staged' },
  { value: 'restored', label: 'restored' },
  { value: 'dismissed', label: 'dismissed' },
  { value: 'scheduled', label: 'scheduled' },
  { value: 'deleted', label: 'deleted' },
  { value: 'deferred', label: 'deferred' },
  { value: 'halted', label: 'halted' },
  { value: 'restaged_recrawled', label: 'restaged (recrawled)' },
  { value: 'scan_started', label: 'scan started' },
  { value: 'scan_finished', label: 'scan finished' },
];

function actionTone(action: string): BadgeTone {
  switch (action) {
    case 'deleted':
    case 'halted':
    case 'delete_failed':
    case 'scan_failed':
      return 'rose';
    case 'staged':
    case 'scheduled':
    case 'deferred':
    case 'restaged_recrawled':
      return 'gold';
    case 'restored':
    case 'reaper_resumed':
    case 'scan_finished':
      return 'mint';
    default:
      return 'neutral';
  }
}

/** The keys worth surfacing inline, in reading order. */
function detailSummary(detail: Record<string, unknown> | null): string {
  if (!detail) return '';
  const parts: string[] = [];
  if (typeof detail.count === 'number') parts.push(`${formatCount(detail.count)} documents`);
  if (typeof detail.reason === 'string') parts.push(detail.reason);
  if (typeof detail.chunks_deleted === 'number')
    parts.push(`${formatCount(detail.chunks_deleted)} chunks deleted`);
  if (detail.remember === true) parts.push('remembered');
  if (detail.expedited === true) parts.push('deadline moved to now');
  if (typeof detail.via === 'string') parts.push(`via ${detail.via}`);
  if (typeof detail.error === 'string') parts.push(detail.error);
  return parts.join(' · ');
}

export function HistoryTab() {
  const [action, setAction] = useState('');
  const [documentFilter, setDocumentFilter] = useState('');
  const [page, setPage] = useState(1);
  const debouncedDocument = useDebouncedValue(documentFilter, 300);

  const params = useMemo(
    () => ({
      action: action || undefined,
      document_id: debouncedDocument.trim() || undefined,
      limit: 50,
      page,
    }),
    [action, debouncedDocument, page],
  );
  const audit = useQuery(pruneAuditQuery(params));

  if (audit.isError) {
    return <ErrorState error={audit.error} onRetry={() => void audit.refetch()} />;
  }

  return (
    <Card className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={action}
          onValueChange={(value) => {
            setAction(value);
            setPage(1);
          }}
          options={ACTION_OPTIONS}
          ariaLabel="Filter by action"
        />
        <Input
          value={documentFilter}
          onChange={(event) => {
            setDocumentFilter(event.target.value);
            setPage(1);
          }}
          placeholder="filter by exact document id"
          aria-label="Filter by document id"
          className="min-w-64 flex-1 font-mono"
        />
      </div>

      {audit.isPending ? (
        <div className="space-y-2">
          <Skeleton className="h-9" />
          <Skeleton className="h-9" />
          <Skeleton className="h-9" />
        </div>
      ) : audit.data.items.length === 0 ? (
        <EmptyState
          icon={<ScrollText aria-hidden />}
          title="No audit entries match"
          description="Every stage, restore, dismissal, schedule and deletion lands here, with who and when."
        />
      ) : (
        <>
          <ul className="divide-y divide-line">
            {audit.data.items.map((entry) => (
              <AuditRow key={entry.id} entry={entry} />
            ))}
          </ul>
          <div className="flex items-center justify-between text-label text-ink-mute">
            <span>
              page {page} · {formatCount(audit.data.total)} entries
            </span>
            <span className="flex gap-2">
              <Button size="sm" disabled={page <= 1} onClick={() => setPage(page - 1)}>
                Previous
              </Button>
              <Button size="sm" disabled={!audit.data.has_more} onClick={() => setPage(page + 1)}>
                Next
              </Button>
            </span>
          </div>
        </>
      )}
    </Card>
  );
}

function AuditRow({ entry }: { entry: PruneAuditItem }) {
  const summary = detailSummary(entry.detail);
  const cleanupPending = entry.detail?.index_cleanup_pending === true;
  return (
    <li className="flex flex-wrap items-center gap-2 py-2">
      <span className="w-24 shrink-0 text-caption text-ink-faint" title={entry.at}>
        {relative(entry.at)}
      </span>
      <Badge tone={actionTone(entry.action)}>{entry.action.replaceAll('_', ' ')}</Badge>
      <span className="text-caption text-ink-mute">{entry.actor}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-caption text-ink-mute">
        {entry.document_id ?? (entry.scan_id !== null ? `scan ${entry.scan_id}` : '')}
      </span>
      {cleanupPending ? <Badge tone="gold">index cleanup pending</Badge> : null}
      {summary ? <span className="w-full pl-24 text-caption text-ink-faint md:w-auto md:pl-0">{summary}</span> : null}
    </li>
  );
}
