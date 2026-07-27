import type { ComponentProps, ReactNode } from 'react';
import { DropdownMenu } from 'radix-ui';
import { cn } from '@/lib/cn';

/** Styled Radix dropdown — real menu semantics (roles, arrows, typeahead). */

export const MenuRoot = DropdownMenu.Root;
export const MenuTrigger = DropdownMenu.Trigger;

export function MenuContent({
  className,
  children,
  ...props
}: ComponentProps<typeof DropdownMenu.Content>) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        sideOffset={6}
        collisionPadding={12}
        className={cn(
          'glass-panel z-50 min-w-48 rounded-xl p-1.5 animate-scale-in',
          className,
        )}
        {...props}
      >
        {children}
      </DropdownMenu.Content>
    </DropdownMenu.Portal>
  );
}

export function MenuItem({
  className,
  destructive,
  icon,
  children,
  ...props
}: ComponentProps<typeof DropdownMenu.Item> & {
  destructive?: boolean;
  icon?: ReactNode;
}) {
  return (
    <DropdownMenu.Item
      className={cn(
        'flex min-h-11 md:min-h-8 cursor-default items-center gap-2.5 rounded-lg px-2.5 text-body md:text-label outline-none select-none',
        destructive
          ? 'text-rose data-highlighted:bg-rose/15'
          : 'text-ink data-highlighted:bg-hover',
        'data-disabled:pointer-events-none data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      {icon ? <span className="text-ink-faint [&>svg]:size-4">{icon}</span> : null}
      {children}
    </DropdownMenu.Item>
  );
}

export function MenuSeparator({ className, ...props }: ComponentProps<typeof DropdownMenu.Separator>) {
  return (
    <DropdownMenu.Separator className={cn('my-1 h-px bg-line-2', className)} {...props} />
  );
}

export function MenuLabel({ className, ...props }: ComponentProps<typeof DropdownMenu.Label>) {
  return (
    <DropdownMenu.Label
      className={cn('px-2.5 py-1.5 text-caption text-ink-faint', className)}
      {...props}
    />
  );
}
