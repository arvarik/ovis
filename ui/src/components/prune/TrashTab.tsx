/**
 * Trash — deleted documents Onyx cannot see and OVIS can put back.
 *
 * The countdown is the point of this screen. Everything here is recoverable
 * until its retention ends, and the moment it stops being recoverable is shown
 * on every row rather than left to be worked out from a policy setting.
 *
 * Restore is one click, because it is the safe direction. Purge is the only
 * irreversible action in the whole product, so it asks for the count to be
 * typed at any size and refuses to touch anything on hold. There is no
 * "empty trash" button, deliberately.
 */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { trashDetailQuery, trashQuery } from '@/api/queries';
import { useTrashHold, useTrashPurge, useTrashRestore } from '@/api/mutations';
import type { TrashItem } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Checkbox } from '@/components/primitives/Checkbox';
import { Dialog } from '@/components/primitives/Dialog';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Sheet } from '@/components/primitives/Sheet';
import { Skeleton } from '@/components/primitives/Skeleton';
import { bytes as formatBytes, count as formatCount } from '@/lib/format';
import { graceCountdown, useNow } from './pruneShared';

export function TrashTab() {
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [inspecting, setInspecting] = useState<string | null>(null);
  const [purging, setPurging] = useState(false);
  const now = useNow();

  const trash = useQuery(trashQuery({ limit: 50, page }));
  const restore = useTrashRestore();
  const hold = useTrashHold();

  const items = trash.data?.items ?? [];
  const total = trash.data?.total ?? 0;
  const selectedItems = items.filter((item) => selected.has(item.document_id));

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  if (trash.isPending) return <Skeleton className="h-64 w-full" />;
  if (trash.isError) {
    return (
      <EmptyState
        title="Trash unavailable"
        description={trash.error?.message ?? 'The trash could not be loaded.'}
      />
    );
  }

  if (total === 0) {
    return (
      <EmptyState
        title="Nothing in the trash"
        description="Deleted documents land here with their content, metadata and embedding vectors, and stay restorable until their retention ends."
      />
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-label text-ink-mute">
        These documents are gone from Onyx — search, connectors and the admin UI have no
        record of them. OVIS still holds a complete snapshot, so restoring puts back the text,
        the tags, the connector attribution and the embedding vectors.
      </p>

      {selectedItems.length > 0 ? (
        <Card className="flex flex-wrap items-center gap-2 p-3">
          <span className="text-label text-ink">
            {formatCount(selectedItems.length)} selected
          </span>
          <Button
            size="sm"
            disabled={restore.isPending}
            onClick={() =>
              restore.mutate(
                {
                  document_ids: selectedItems.map((i) => i.document_id),
                  confirm_count: selectedItems.length,
                  overwrite: false,
                },
                { onSuccess: () => setSelected(new Set()) },
              )
            }
          >
            Restore
          </Button>
          <Button
            size="sm"
            variant="secondary"
            disabled={hold.isPending}
            onClick={() =>
              hold.mutate({
                document_ids: selectedItems.map((i) => i.document_id),
                hold: !selectedItems.every((i) => i.hold),
              })
            }
          >
            {selectedItems.every((i) => i.hold) ? 'Release hold' : 'Hold indefinitely'}
          </Button>
          <Button size="sm" variant="destructive" onClick={() => setPurging(true)}>
            Destroy permanently…
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
            Clear
          </Button>
        </Card>
      ) : null}

      <Card className="overflow-x-auto">
        <table className="w-full text-label">
          <thead className="text-ink-mute">
            <tr className="border-b border-line">
              <th className="w-8 p-2" />
              <th className="p-2 text-left font-normal">Document</th>
              <th className="p-2 text-left font-normal">Deleted</th>
              <th className="p-2 text-left font-normal">Recoverable for</th>
              <th className="p-2 text-right font-normal">Snapshot</th>
              <th className="w-24 p-2" />
            </tr>
          </thead>
          <tbody>
            {items.map((item) => (
              <TrashRow
                key={item.document_id}
                item={item}
                now={now}
                checked={selected.has(item.document_id)}
                onToggle={() => toggle(item.document_id)}
                onInspect={() => setInspecting(item.document_id)}
              />
            ))}
          </tbody>
        </table>
      </Card>

      {total > 50 ? (
        <div className="flex items-center justify-between">
          <Button
            size="sm"
            variant="secondary"
            disabled={page === 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            Previous
          </Button>
          <span className="text-caption text-ink-mute">
            {formatCount(total)} snapshots
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={!trash.data?.has_more}
            onClick={() => setPage((p) => p + 1)}
          >
            Next
          </Button>
        </div>
      ) : null}

      {inspecting ? (
        <SnapshotSheet documentId={inspecting} onClose={() => setInspecting(null)} />
      ) : null}

      {purging ? (
        <PurgeDialog
          items={selectedItems}
          onClose={() => setPurging(false)}
          onPurged={() => {
            setSelected(new Set());
            setPurging(false);
          }}
        />
      ) : null}
    </div>
  );
}

function TrashRow({
  item,
  now,
  checked,
  onToggle,
  onInspect,
}: {
  item: TrashItem;
  now: number;
  checked: boolean;
  onToggle: () => void;
  onInspect: () => void;
}) {
  const restore = useTrashRestore();
  const remaining = graceCountdown(item.expires_at, now);
  const expired = new Date(item.expires_at).getTime() <= now;

  return (
    <tr className="border-b border-line/50">
      <td className="p-2">
        <Checkbox
          checked={checked}
          onCheckedChange={onToggle}
          label={`Select ${item.document_id}`}
          stopPropagation
        />
      </td>
      <td className="p-2">
        <button type="button" onClick={onInspect} className="text-left text-ink hover:underline">
          {item.semantic_id ?? item.document_id}
        </button>
        <div className="text-caption text-ink-mute">
          {item.connector_name ?? '—'} · {formatCount(item.chunk_count)} chunks
          {item.vectors_included ? ' · vectors kept' : ' · no vectors'}
        </div>
        {item.reappeared ? (
          <Badge tone="gold" className="mt-1">
            back in Onyx — restoring would collide
          </Badge>
        ) : null}
      </td>
      <td className="p-2 text-ink-mute">
        <div>{new Date(item.deleted_at).toLocaleDateString()}</div>
        <div className="text-caption">by {item.deleted_by}</div>
      </td>
      <td className="p-2">
        {item.hold ? (
          <Badge tone="indigo">held — never auto-purged</Badge>
        ) : expired ? (
          <Badge tone="rose">purging shortly</Badge>
        ) : (
          <span className="text-ink">{remaining}</span>
        )}
      </td>
      <td className="p-2 text-right tabular-nums text-ink-mute">
        {formatBytes(item.snapshot_bytes)}
      </td>
      <td className="p-2 text-right">
        <Button
          size="sm"
          variant="secondary"
          disabled={restore.isPending}
          onClick={() =>
            restore.mutate({
              document_ids: [item.document_id],
              confirm_count: 1,
              overwrite: item.reappeared,
            })
          }
        >
          {item.reappeared ? 'Overwrite' : 'Restore'}
        </Button>
      </td>
    </tr>
  );
}

function SnapshotSheet({ documentId, onClose }: { documentId: string; onClose: () => void }) {
  const detail = useQuery(trashDetailQuery(documentId));

  return (
    <Sheet open onOpenChange={(open) => !open && onClose()} title="Snapshot">
      {detail.isPending ? <Skeleton className="h-48 w-full" /> : null}
      {detail.data ? (
        <div className="space-y-4">
          <div>
            <div className="font-display text-title text-ink">
              {detail.data.semantic_id ?? detail.data.document_id}
            </div>
            <div className="mt-1 break-all text-caption text-ink-mute">
              {detail.data.document_id}
            </div>
          </div>

          <div className="rounded-md border border-line bg-surface-2 p-3 text-caption text-ink-mute">
            This is the snapshot, not a live document. Onyx has no record of it — what you are
            reading is the copy OVIS kept so it could be restored.
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Deleted" value={new Date(detail.data.deleted_at).toLocaleString()} />
            <Field label="Retention ends" value={new Date(detail.data.expires_at).toLocaleString()} />
            <Field label="Chunks" value={formatCount(detail.data.chunk_count)} />
            <Field label="Snapshot size" value={formatBytes(detail.data.snapshot_bytes)} />
          </div>

          {detail.data.reasons && detail.data.reasons.length > 0 ? (
            <div>
              <div className="text-label text-ink-mute">Why it was pruned</div>
              <ul className="mt-1 space-y-1">
                {detail.data.reasons.map((reason) => (
                  <li key={`${reason.detector}-${reason.code}`} className="text-caption text-ink">
                    {reason.detail}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          <div>
            <div className="text-label text-ink-mute">Content</div>
            <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap rounded-md border border-line bg-surface-2 p-3 text-caption text-ink">
              {detail.data.text || '(no extractable text — this document had none)'}
            </pre>
          </div>
        </div>
      ) : null}
    </Sheet>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="text-label text-ink-mute">{label}</div>
      <div className="text-ink">{value}</div>
    </div>
  );
}

/**
 * The one dialog in the product that guards a genuinely irreversible action.
 * It asks for the count typed back regardless of how small the selection is,
 * because "only three" is exactly when people stop reading.
 */
function PurgeDialog({
  items,
  onClose,
  onPurged,
}: {
  items: TrashItem[];
  onClose: () => void;
  onPurged: () => void;
}) {
  const [typed, setTyped] = useState('');
  const purge = useTrashPurge();
  const held = items.filter((i) => i.hold);
  const matches = Number(typed) === items.length;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title="Destroy permanently">
      <div className="space-y-3">
        <p className="text-label text-ink">
          This destroys {formatCount(items.length)} snapshot
          {items.length === 1 ? '' : 's'}. The documents are already gone from Onyx; this
          removes the only copy that could bring them back.
        </p>
        <p className="text-label text-rose">There is no undo for this action.</p>

        {held.length > 0 ? (
          <p className="text-caption text-gold">
            {formatCount(held.length)} of these are on hold and will be skipped. Release the
            hold first if you really mean to destroy them.
          </p>
        ) : null}

        <label className="block text-label text-ink-mute">
          Type {items.length} to confirm
          <Input
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            inputMode="numeric"
            autoFocus
            className="mt-1"
            aria-label={`Type ${items.length} to confirm`}
          />
        </label>

        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={!matches || purge.isPending}
            onClick={() =>
              purge.mutate(
                {
                  document_ids: items.map((i) => i.document_id),
                  confirm_count: items.length,
                  typed_count: items.length,
                },
                { onSuccess: onPurged },
              )
            }
          >
            {purge.isPending ? 'Destroying…' : 'Destroy permanently'}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
