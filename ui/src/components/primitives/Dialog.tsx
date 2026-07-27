import type { ReactNode } from 'react';
import { Dialog as RadixDialog, AlertDialog as RadixAlertDialog } from 'radix-ui';
import { X } from 'lucide-react';
import { cn } from '@/lib/cn';
import { useHotkeyLayer } from '@/hooks/hotkeys';
import { IconButton } from './Button';

/**
 * Centered glass modal (desktop) that hugs the bottom on small screens.
 * Radix supplies focus trap, aria-modal, scroll lock and Esc priority.
 */
export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  hideTitle,
  className,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
  hideTitle?: boolean;
  className?: string;
}) {
  useHotkeyLayer('dialog', open);
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-50 bg-black/60 animate-fade-in" />
        <RadixDialog.Content
          className={cn(
            'glass-panel fixed z-50 flex flex-col animate-scale-in outline-none',
            'inset-x-0 bottom-0 max-h-[90dvh] rounded-t-2xl',
            'md:inset-x-auto md:bottom-auto md:top-1/2 md:left-1/2 md:max-h-[85dvh] md:w-full md:max-w-lg md:-translate-x-1/2 md:-translate-y-1/2 md:rounded-2xl',
            className,
          )}
        >
          <div className={cn('flex items-start justify-between gap-3 px-5 pt-4', hideTitle && 'sr-only')}>
            <div>
              <RadixDialog.Title className="font-display font-display-soft text-title text-ink">
                {title}
              </RadixDialog.Title>
              {description ? (
                <RadixDialog.Description className="mt-0.5 text-label text-ink-mute">
                  {description}
                </RadixDialog.Description>
              ) : null}
            </div>
            <RadixDialog.Close asChild>
              <IconButton label="Close" className="-mr-2 -mt-1">
                <X className="size-4" aria-hidden />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-5 pt-3">{children}</div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

/**
 * Confirmation dialog for destructive/irreversible actions. Consequences are
 * spelled out by the caller in `children`; the confirm button never defaults
 * to focused (Radix focuses Cancel first).
 */
export function AlertDialog({
  open,
  onOpenChange,
  title,
  children,
  cancelLabel = 'Cancel',
  actions,
  className,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  children: ReactNode;
  cancelLabel?: string;
  /** Action buttons (already wired); rendered after Cancel. */
  actions: ReactNode;
  className?: string;
}) {
  useHotkeyLayer('dialog', open);
  return (
    <RadixAlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixAlertDialog.Portal>
        <RadixAlertDialog.Overlay className="fixed inset-0 z-50 bg-black/60 animate-fade-in" />
        <RadixAlertDialog.Content
          className={cn(
            'glass-panel fixed z-50 flex flex-col animate-scale-in outline-none',
            'inset-x-0 bottom-0 max-h-[90dvh] rounded-t-2xl',
            'md:inset-x-auto md:bottom-auto md:top-1/2 md:left-1/2 md:max-h-[85dvh] md:w-full md:max-w-md md:-translate-x-1/2 md:-translate-y-1/2 md:rounded-2xl',
            className,
          )}
        >
          <div className="min-h-0 flex-1 overflow-y-auto px-5 pt-4">
            <RadixAlertDialog.Title className="font-display font-display-soft text-title text-ink">
              {title}
            </RadixAlertDialog.Title>
            <div className="mt-2 text-body text-ink-mute">{children}</div>
          </div>
          <div className="flex flex-col-reverse gap-2 px-5 py-4 sm:flex-row sm:justify-end">
            <RadixAlertDialog.Cancel asChild>
              <button
                type="button"
                className="inline-flex min-h-11 md:min-h-9 items-center justify-center rounded-lg border border-line-2 bg-surface px-4 text-body md:text-label text-ink transition-colors hover:bg-hover"
              >
                {cancelLabel}
              </button>
            </RadixAlertDialog.Cancel>
            {actions}
          </div>
        </RadixAlertDialog.Content>
      </RadixAlertDialog.Portal>
    </RadixAlertDialog.Root>
  );
}
