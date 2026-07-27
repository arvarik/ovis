import { Select as RadixSelect } from 'radix-ui';
import { Check, ChevronDown } from 'lucide-react';
import { cn } from '@/lib/cn';

export interface SelectOption {
  value: string;
  label: string;
}

/** Radix can't represent an empty-string item value; map '' through a sentinel. */
const NONE = '__none__';

/**
 * Token-styled dropdown (replaces native `<select>`, which renders as the
 * OS default). Keyboard/typeahead/scroll behaviour comes from Radix; the
 * trigger matches Input, the menu is level-2 glass.
 */
export function Select({
  value,
  onValueChange,
  options,
  ariaLabel,
  className,
}: {
  value: string;
  onValueChange: (value: string) => void;
  /** An option with value '' renders as the "any/none" choice. */
  options: SelectOption[];
  ariaLabel: string;
  className?: string;
}) {
  return (
    <RadixSelect.Root
      value={value === '' ? NONE : value}
      onValueChange={(v) => onValueChange(v === NONE ? '' : v)}
    >
      <RadixSelect.Trigger
        aria-label={ariaLabel}
        className={cn(
          'flex min-h-11 w-full items-center justify-between gap-2 rounded-lg border border-line bg-well px-3 text-left text-base text-ink md:min-h-9 md:text-body',
          'focus:border-gold/60 focus:ring-2 focus:ring-gold/20 focus:outline-none',
          'data-[state=open]:border-gold/60',
          'disabled:pointer-events-none disabled:opacity-50',
          className,
        )}
      >
        <span className="truncate">
          <RadixSelect.Value />
        </span>
        <RadixSelect.Icon>
          <ChevronDown className="size-4 shrink-0 text-ink-faint" aria-hidden />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content
          position="popper"
          sideOffset={6}
          collisionPadding={12}
          className="glass-panel z-50 max-h-[min(20rem,var(--radix-select-content-available-height))] w-[var(--radix-select-trigger-width)] min-w-44 overflow-hidden rounded-xl animate-scale-in"
        >
          <RadixSelect.Viewport className="p-1.5">
            {options.map((opt) => (
              <RadixSelect.Item
                key={opt.value}
                value={opt.value === '' ? NONE : opt.value}
                className={cn(
                  'flex min-h-11 cursor-default items-center gap-2 rounded-lg px-2.5 text-body outline-none select-none md:min-h-8 md:text-label',
                  'text-ink data-highlighted:bg-hover',
                  'data-[state=checked]:text-gold',
                )}
              >
                <span className="flex w-4 shrink-0 items-center justify-center">
                  <RadixSelect.ItemIndicator>
                    <Check className="size-3.5" aria-hidden />
                  </RadixSelect.ItemIndicator>
                </span>
                <RadixSelect.ItemText>
                  <span className="truncate">{opt.label}</span>
                </RadixSelect.ItemText>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}
