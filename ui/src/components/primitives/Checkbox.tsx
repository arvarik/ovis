import { Checkbox as RadixCheckbox } from 'radix-ui';
import { Check } from 'lucide-react';
import { cn } from '@/lib/cn';

export function Checkbox({
  checked,
  onCheckedChange,
  label,
  className,
  stopPropagation,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  /** Accessible name — checkboxes never go unlabeled. */
  label: string;
  className?: string;
  /** Row checkboxes must not also trigger the row click. */
  stopPropagation?: boolean;
}) {
  return (
    <RadixCheckbox.Root
      checked={checked}
      onCheckedChange={(v) => onCheckedChange(v === true)}
      aria-label={label}
      onClick={stopPropagation ? (e) => e.stopPropagation() : undefined}
      className={cn(
        'flex size-4.5 shrink-0 items-center justify-center rounded border transition-colors',
        checked ? 'border-gold bg-gold text-canvas' : 'border-line-3 bg-well hover:border-gold/50',
        className,
      )}
    >
      <RadixCheckbox.Indicator>
        <Check className="size-3.5" strokeWidth={3} aria-hidden />
      </RadixCheckbox.Indicator>
    </RadixCheckbox.Root>
  );
}
