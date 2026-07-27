import type { ReactNode } from 'react';
import { Drawer } from 'vaul';
import { cn } from '@/lib/cn';
import { useIsDesktop } from '@/hooks/useMediaQuery';
import { useHotkeyLayer } from '@/hooks/hotkeys';

/**
 * The single overlay-surface primitive (P5): a bottom sheet on mobile and a
 * right side panel on desktop — one component, `direction` decided by media
 * query, never two component trees.
 */
export function Sheet({
  open,
  onOpenChange,
  title,
  description,
  children,
  desktopWidth = 'md:max-w-3xl',
  dismissible = true,
  contentClassName,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Accessible name; rendered sr-only (content usually shows its own header). */
  title: string;
  description?: string;
  children: ReactNode;
  desktopWidth?: string;
  dismissible?: boolean;
  contentClassName?: string;
}) {
  const isDesktop = useIsDesktop();
  useHotkeyLayer('sheet', open);

  return (
    <Drawer.Root
      open={open}
      onOpenChange={onOpenChange}
      direction={isDesktop ? 'right' : 'bottom'}
      dismissible={dismissible}
    >
      <Drawer.Portal>
        <Drawer.Overlay className="fixed inset-0 z-40 bg-black/60" />
        <Drawer.Content
          className={cn(
            'fixed z-50 flex flex-col bg-surface outline-none',
            isDesktop
              ? cn('inset-y-0 right-0 w-full border-l border-line-2', desktopWidth)
              : 'inset-x-0 bottom-0 max-h-[94dvh] rounded-t-2xl border-t border-line-2',
            contentClassName,
          )}
        >
          {!isDesktop ? (
            <div
              aria-hidden
              className="mx-auto mt-2.5 mb-1 h-1 w-10 shrink-0 rounded-full bg-line-3"
            />
          ) : null}
          <Drawer.Title className="sr-only">{title}</Drawer.Title>
          {description ? (
            <Drawer.Description className="sr-only">{description}</Drawer.Description>
          ) : null}
          {children}
        </Drawer.Content>
      </Drawer.Portal>
    </Drawer.Root>
  );
}
