import type { ReactNode } from 'react';
import { Tooltip as RadixTooltip } from 'radix-ui';

export const TooltipProvider = RadixTooltip.Provider;

/** Hover/focus hint. Touch devices never rely on it — always pair with a visible label or aria. */
export function Tooltip({
  content,
  children,
  side = 'top',
}: {
  content: ReactNode;
  children: ReactNode;
  side?: 'top' | 'bottom' | 'left' | 'right';
}) {
  return (
    <RadixTooltip.Root delayDuration={350}>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          sideOffset={6}
          collisionPadding={12}
          className="glass-panel z-50 max-w-xs rounded-lg px-2.5 py-1.5 text-caption text-ink animate-scale-in"
        >
          {content}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}
