import { useState } from 'react';
import { Minus, Plus } from 'lucide-react';
import { Switch } from 'radix-ui';
import type { PageDetail, PagePatch } from '@/api/types';
import { usePatchPage } from '@/api/mutations';
import { cn } from '@/lib/cn';
import { Button, IconButton } from '@/components/primitives/Button';
import { Dialog } from '@/components/primitives/Dialog';
import { Input } from '@/components/primitives/Input';

const BOOST_MIN = -4;
const BOOST_MAX = 8;

export function EditSheet({
  detail,
  open,
  onOpenChange,
}: {
  detail: PageDetail;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [title, setTitle] = useState(detail.semantic_id);
  const [boost, setBoost] = useState(detail.boost);
  const [hidden, setHidden] = useState(detail.hidden);
  const patch = usePatchPage(detail.id);

  const changes: PagePatch = {};
  if (title !== detail.semantic_id && title.trim() !== '') changes.semantic_id = title;
  if (boost !== detail.boost) changes.boost = boost;
  if (hidden !== detail.hidden) changes.hidden = hidden;
  const dirty = Object.keys(changes).length > 0;

  const save = () => {
    patch.mutate(changes, {
      onSuccess: () => onOpenChange(false),
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title="Edit document">
      <div className="space-y-5">
        <label className="flex flex-col gap-1.5">
          <span className="text-label text-ink-mute">Title</span>
          <Input value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>

        <div className="flex flex-col gap-1.5">
          <span className="text-label text-ink-mute">
            Boost <span className="text-ink-faint">— affects Onyx ranking</span>
          </span>
          <div className="flex items-center gap-3">
            <IconButton
              label="Decrease boost"
              variant="secondary"
              disabled={boost <= BOOST_MIN}
              onClick={() => setBoost((b) => Math.max(BOOST_MIN, b - 1))}
            >
              <Minus className="size-4" aria-hidden />
            </IconButton>
            <span className="stat-numeral w-10 text-center text-title text-ink">
              {boost > 0 ? `+${boost}` : boost}
            </span>
            <IconButton
              label="Increase boost"
              variant="secondary"
              disabled={boost >= BOOST_MAX}
              onClick={() => setBoost((b) => Math.min(BOOST_MAX, b + 1))}
            >
              <Plus className="size-4" aria-hidden />
            </IconButton>
          </div>
        </div>

        <label className="flex items-center justify-between gap-3">
          <span className="text-label text-ink-mute">
            Hidden
            <span className="block text-caption text-ink-faint">
              hides from Onyx search results, keeps all data
            </span>
          </span>
          <Switch.Root
            checked={hidden}
            onCheckedChange={setHidden}
            className={cn(
              'relative h-6 w-11 shrink-0 rounded-full border transition-colors',
              hidden ? 'border-gold/50 bg-gold/80' : 'border-line-3 bg-well',
            )}
          >
            <Switch.Thumb
              className={cn(
                'block size-5 translate-x-0.5 rounded-full bg-ink transition-transform',
                hidden && 'translate-x-5.5 bg-canvas',
              )}
            />
          </Switch.Root>
        </label>

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!dirty || patch.isPending} onClick={save}>
            {patch.isPending ? 'Saving…' : 'Save'}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
