import { AlertDialog } from '@/components/primitives/Dialog';
import { Button } from '@/components/primitives/Button';
import { useDeletePage, useHidePages } from '@/api/mutations';
import type { PageDetail } from '@/api/types';
import { count as formatCount } from '@/lib/format';

/**
 * Delete is hard and permanent — there is no undo (the old UI's was fake).
 * Consequences are spelled out from the API's own honest fields, and the
 * reversible alternative (hide) is offered right here.
 */
export function DeleteDialog({
  detail,
  open,
  onOpenChange,
  onDeleted,
}: {
  detail: PageDetail;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDeleted: () => void;
}) {
  const del = useDeletePage();
  const hide = useHidePages();

  return (
    <AlertDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Delete this document?"
      actions={
        <>
          {!detail.hidden ? (
            <Button
              variant="secondary"
              disabled={hide.isPending || del.isPending}
              onClick={() =>
                hide.mutate(
                  { ids: [detail.id], hidden: true },
                  { onSuccess: () => onOpenChange(false) },
                )
              }
            >
              Hide from search instead
            </Button>
          ) : null}
          <Button
            variant="destructive"
            disabled={del.isPending}
            onClick={() =>
              del.mutate(detail.id, {
                onSuccess: () => {
                  onOpenChange(false);
                  onDeleted();
                },
              })
            }
          >
            {del.isPending ? 'Deleting…' : 'Delete permanently'}
          </Button>
        </>
      }
    >
      <div className="space-y-2">
        <p className="break-all">
          <span className="font-mono text-mono-sm text-ink">{detail.link ?? detail.id}</span>
        </p>
        <p>
          {detail.chunk_count === null
            ? 'Its chunk count has not been recorded yet — the index will be swept by id.'
            : `${formatCount(detail.chunk_count)} chunk${detail.chunk_count === 1 ? '' : 's'} will be removed from the search index.`}{' '}
          The deletion is immediate and permanent — there is no undo.
        </p>
        {detail.recrawl_risk ? (
          <p className="rounded-lg border border-gold/30 bg-gold/10 px-3 py-2 text-label text-gold">
            The owning connector is {detail.cc_pair_status ?? 'ACTIVE'} — this page will likely be
            re-crawled on its next scheduled refresh and reappear.
          </p>
        ) : null}
      </div>
    </AlertDialog>
  );
}
