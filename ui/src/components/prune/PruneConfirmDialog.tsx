/**
 * The stage / schedule-delete confirmation: count, chunk sum, recrawl-risk
 * breakdown, and where restore lives. Past the server's big-batch threshold
 * the exact count must be typed — there is no one-click path (02 §5).
 */
import { useState } from 'react';
import { Dialog } from '@/components/primitives/Dialog';
import { Button } from '@/components/primitives/Button';
import { Input } from '@/components/primitives/Input';
import { count as formatCount } from '@/lib/format';
import { needsTypedCount } from './pruneShared';

export interface PruneConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** "Stage" | "Schedule deletion of" — the verb phrase. */
  verb: string;
  /** Server-truth selection size; sent as confirm_count. */
  total: number;
  /** Chunk sum over the rows in view (may be a sample of the selection). */
  chunkSum: number;
  /** Whether chunkSum covers every selected row or only the loaded page. */
  chunkSumComplete: boolean;
  /**
   * Documents in the selection whose connector is still crawling. `null`
   * while the server is still counting — the dialog says so rather than
   * claiming zero, which for a filtered selection would be a claim about the
   * loaded page dressed up as a claim about the whole set.
   */
  riskyCount: number | null;
  bigBatch: number;
  graceDays: number;
  /** What this action leads to, e.g. "They stay restorable until the grace ends." */
  consequence: string;
  confirmLabel: string;
  destructive?: boolean;
  pending?: boolean;
  onConfirm: () => void;
}

export function PruneConfirmDialog(props: PruneConfirmProps) {
  const {
    open,
    onOpenChange,
    verb,
    total,
    chunkSum,
    chunkSumComplete,
    riskyCount,
    bigBatch,
    graceDays,
    consequence,
    confirmLabel,
    destructive,
    pending,
    onConfirm,
  } = props;
  const [typed, setTyped] = useState('');

  const typedRequired = needsTypedCount(total, bigBatch);
  const typedOk = !typedRequired || typed.trim() === String(total);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // The typed count never carries over into a later confirmation.
        if (!next) setTyped('');
        onOpenChange(next);
      }}
      title={`${verb} ${formatCount(total)} document${total === 1 ? '' : 's'}?`}
    >
      <div className="space-y-3 text-body text-ink">
        <ul className="space-y-1 text-label text-ink-mute">
          <li>
            {chunkSumComplete
              ? `${formatCount(chunkSum)} chunk${chunkSum === 1 ? '' : 's'} in the search index`
              : `${formatCount(chunkSum)} chunks across the rows loaded so far — the full selection holds more`}
          </li>
          {riskyCount === null ? (
            <li>counting how many are at recrawl risk…</li>
          ) : riskyCount > 0 ? (
            <li className="text-gold">
              {formatCount(riskyCount)} belong to connectors that are still crawling — a deleted
              copy will likely be re-crawled at the next refresh
            </li>
          ) : (
            <li>none are at recrawl risk</li>
          )}
          <li>
            grace period: {graceDays} day{graceDays === 1 ? '' : 's'} · restore lives in the
            Staged tab until it ends
          </li>
        </ul>
        <p className="text-label text-ink-mute">{consequence}</p>

        {typedRequired ? (
          <div className="space-y-1">
            <label className="text-label text-ink-mute" htmlFor="prune-confirm-count">
              This is more than {formatCount(bigBatch)} documents. Type the count ({total}) to
              continue:
            </label>
            {/* No placeholder: the ceremony is reading the number above and
                typing it deliberately, which a pre-filled-looking field
                would undo. */}
            <Input
              id="prune-confirm-count"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              inputMode="numeric"
              autoComplete="off"
            />
          </div>
        ) : null}

        <div className="flex justify-end gap-2 pt-1">
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button
            variant={destructive ? 'destructive' : 'primary'}
            disabled={!typedOk || pending}
            onClick={onConfirm}
          >
            {confirmLabel}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
