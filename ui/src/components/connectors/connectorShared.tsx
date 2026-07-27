import { useState } from 'react';
import {
  CircleParking,
  Pause,
  Pencil,
  Play,
  Scissors,
  Trash2,
} from 'lucide-react';
import type { ConnectorSummary } from '@/api/types';
import {
  useConnectorDelete,
  useConnectorPatch,
  usePauseResume,
  useConnectorPrune,
  useRunOnce,
} from '@/api/mutations';
import { cn } from '@/lib/cn';
import { count as formatCount } from '@/lib/format';
import { statusTone, type BadgeTone } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { AlertDialog, Dialog } from '@/components/primitives/Dialog';
import { Input } from '@/components/primitives/Input';
import { MenuItem, MenuSeparator } from '@/components/primitives/Menu';

export const TONE_DOT: Record<BadgeTone, string> = {
  mint: 'bg-mint',
  gold: 'bg-gold',
  rose: 'bg-rose',
  indigo: 'bg-indigo',
  violet: 'bg-violet',
  teal: 'bg-teal',
  neutral: 'bg-ink-faint',
};

export function StatusDot({ status, className }: { status: string; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn('size-2 shrink-0 rounded-full', TONE_DOT[statusTone(status)], className)}
    />
  );
}

export function ParkedBadge() {
  return (
    <span
      className="inline-flex items-center gap-1 rounded-full border border-gold/30 bg-gold/15 px-1.5 py-0.5 text-caption text-gold"
      title="The resilience cron finished with this cc-pair on purpose (first-pass soft landing)"
    >
      <CircleParking className="size-3" aria-hidden />
      parked
    </span>
  );
}

/**
 * Run-now with the parked guard: a parked cc-pair gets the explainer and an
 * explicit choice — `acknowledge_parked` is never set on the user's behalf.
 */
export function RunOnceDialog({
  connector,
  open,
  onOpenChange,
}: {
  connector: ConnectorSummary;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const runOnce = useRunOnce(connector.cc_pair_id);
  const run = (acknowledge: boolean) =>
    runOnce.mutate(
      connector.parked ? { acknowledge_parked: acknowledge } : {},
      { onSuccess: () => onOpenChange(false) },
    );

  return (
    <AlertDialog
      open={open}
      onOpenChange={onOpenChange}
      title={connector.parked ? 'This connector is parked' : `Crawl ${connector.name} now?`}
      actions={
        <Button
          variant={connector.parked ? 'primary' : 'primary'}
          disabled={runOnce.isPending}
          onClick={() => run(true)}
        >
          {runOnce.isPending
            ? 'Queuing…'
            : connector.parked
              ? 'I understand — run anyway'
              : 'Run now'}
        </Button>
      }
    >
      {connector.parked ? (
        <div className="space-y-2">
          <p>
            The resilience cron finished <span className="text-ink">{connector.name}</span> on
            purpose — its last attempt carries a park sentinel (“first-pass already complete” /
            “park done”). Parked connectors are excluded from automatic un-sticking.
          </p>
          <p>
            Running it again restarts the crawl from its sitemap and may take hours. This is a
            deliberate override, not a retry.
          </p>
        </div>
      ) : (
        <p>
          Queues one indexing attempt via the Onyx API. It may wait behind other in-flight
          attempts before a worker picks it up — a NOT_STARTED attempt is normal queuing, not a
          stall.
        </p>
      )}
    </AlertDialog>
  );
}

export function RenameDialog({
  connector,
  open,
  onOpenChange,
}: {
  connector: ConnectorSummary;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [name, setName] = useState(connector.name);
  const patch = useConnectorPatch(connector.cc_pair_id);
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={`Rename ${connector.name}`}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() === '' || name === connector.name) return;
          patch.mutate({ name: name.trim() }, { onSuccess: () => onOpenChange(false) });
        }}
      >
        <Input value={name} onChange={(e) => setName(e.target.value)} aria-label="Connector name" />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            disabled={patch.isPending || name.trim() === '' || name === connector.name}
          >
            {patch.isPending ? 'Saving…' : 'Rename'}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

/** Deleting a cc-pair requires typing its exact name back — a 100k-doc footgun guard. */
export function DeleteConnectorDialog({
  connector,
  open,
  onOpenChange,
}: {
  connector: ConnectorSummary;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [typed, setTyped] = useState('');
  const del = useConnectorDelete(connector.cc_pair_id);
  const match = typed === connector.name;

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={`Delete ${connector.name}?`}>
      <div className="space-y-4">
        <p className="text-body text-ink-mute">
          This removes the cc-pair and its{' '}
          <span className="text-ink">{formatCount(connector.doc_count)} documents</span> from Onyx
          via a background deletion job. It cannot be undone.
        </p>
        <label className="flex flex-col gap-1.5">
          <span className="text-label text-ink-mute">
            Type <span className="font-mono text-ink select-all">{connector.name}</span> to confirm
          </span>
          <Input
            mono
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            placeholder={connector.name}
          />
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={!match || del.isPending}
            onClick={() => del.mutate(typed, { onSuccess: () => onOpenChange(false) })}
          >
            {del.isPending ? 'Requesting…' : 'Delete connector'}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

export type ConnectorDialogKind = 'run' | 'rename' | 'delete' | null;

/** Shared overflow-menu items; the owning view renders the dialogs. */
export function ConnectorMenuItems({
  connector,
  onDialog,
}: {
  connector: ConnectorSummary;
  onDialog: (kind: Exclude<ConnectorDialogKind, null>) => void;
}) {
  const pauseResume = usePauseResume();
  const prune = useConnectorPrune(connector.cc_pair_id);
  const paused = connector.status === 'PAUSED';

  return (
    <>
      <MenuItem
        icon={paused ? <Play aria-hidden /> : <Pause aria-hidden />}
        onSelect={() =>
          pauseResume.mutate({ ids: [connector.cc_pair_id], action: paused ? 'resume' : 'pause' })
        }
      >
        {paused ? 'Resume' : 'Pause'}
      </MenuItem>
      <MenuItem icon={<Play aria-hidden />} onSelect={() => onDialog('run')}>
        Run now{connector.parked ? ' (parked)' : ''}
      </MenuItem>
      <MenuItem icon={<Scissors aria-hidden />} onSelect={() => prune.mutate()}>
        Prune
      </MenuItem>
      <MenuItem icon={<Pencil aria-hidden />} onSelect={() => onDialog('rename')}>
        Rename
      </MenuItem>
      <MenuSeparator />
      <MenuItem destructive icon={<Trash2 aria-hidden />} onSelect={() => onDialog('delete')}>
        Delete…
      </MenuItem>
    </>
  );
}
