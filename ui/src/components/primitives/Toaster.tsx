import { Toaster as SonnerToaster } from 'sonner';
import { useIsDesktop } from '@/hooks/useMediaQuery';

/**
 * The single toast system (D-finding: the old UI ran two at once).
 * Bottom-right on desktop; bottom-center on mobile, lifted above BottomTabs
 * and the home indicator. Errors persist until dismissed — sonner defaults
 * are overridden per-call in mutations.
 */
export function Toaster() {
  const isDesktop = useIsDesktop();
  return (
    <SonnerToaster
      theme="dark"
      position={isDesktop ? 'bottom-right' : 'bottom-center'}
      offset={16}
      mobileOffset={{ bottom: 'calc(76px + env(safe-area-inset-bottom))' }}
      gap={8}
      toastOptions={{
        className: '!glass-panel !rounded-xl !text-ink !text-body',
        descriptionClassName: '!text-ink-mute',
      }}
    />
  );
}
