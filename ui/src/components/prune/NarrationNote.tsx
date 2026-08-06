/**
 * A generated title and summary, shown as something a model wrote.
 *
 * The whole point of this block is that it is visibly *not* a measurement.
 * Every other number on a prune card was counted; this was written by an LLM
 * from a sample, so it carries the model's name and when it ran, and it sits
 * beside the mechanical description rather than replacing it. A reviewer who
 * disagrees with a title can still see what the detector actually said.
 */
import { Sparkles } from 'lucide-react';
import type { Narration } from '@/api/types';
import { relative } from '@/lib/format';

export function NarrationNote({ narration }: { narration: Narration }) {
  const model = narration.model.split('/').pop() ?? narration.model;
  return (
    <div className="rounded-md border border-line bg-surface-2 p-3">
      <div className="flex items-start gap-2">
        <Sparkles aria-hidden className="mt-0.5 size-3.5 shrink-0 text-gold" />
        <div className="min-w-0">
          <div className="text-ink">{narration.title}</div>
          <p className="mt-1 text-caption text-ink-mute">{narration.summary}</p>
          <p className="mt-1 text-caption text-ink-mute">
            Written by {model} {relative(narration.generated_at)}, from a sample of this
            group's pages — not from every one.
          </p>
        </div>
      </div>
    </div>
  );
}
