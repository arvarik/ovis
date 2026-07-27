import type { ComponentProps } from 'react';
import { Tabs as RadixTabs } from 'radix-ui';
import { cn } from '@/lib/cn';

export const TabsRoot = RadixTabs.Root;

export function TabsList({ className, ...props }: ComponentProps<typeof RadixTabs.List>) {
  return (
    <RadixTabs.List
      className={cn(
        'flex items-center gap-1 overflow-x-auto border-b border-line',
        className,
      )}
      {...props}
    />
  );
}

/** Gold underline = active (palette-consistent: gold is the one selection color). */
export function TabsTrigger({ className, ...props }: ComponentProps<typeof RadixTabs.Trigger>) {
  return (
    <RadixTabs.Trigger
      className={cn(
        'relative min-h-11 md:min-h-9 px-3 text-label text-ink-mute whitespace-nowrap outline-none transition-colors',
        'hover:text-ink',
        'data-[state=active]:text-ink',
        'after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-transparent',
        'data-[state=active]:after:bg-gold',
        'focus-visible:after:bg-gold/50',
        className,
      )}
      {...props}
    />
  );
}

export function TabsContent({ className, ...props }: ComponentProps<typeof RadixTabs.Content>) {
  return <RadixTabs.Content className={cn('outline-none', className)} {...props} />;
}
