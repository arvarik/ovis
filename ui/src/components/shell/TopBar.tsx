import { Link } from '@tanstack/react-router';
import { RefreshCw } from 'lucide-react';
import { useQueryClient, useIsFetching } from '@tanstack/react-query';
import { IconButton } from '@/components/primitives/Button';
import { cn } from '@/lib/cn';
import { HealthDot } from './HealthDot';
import { SearchPill } from './SearchPill';

export function Wordmark() {
  return (
    <Link
      to="/pages"
      className="flex items-center gap-1.5 rounded-lg px-1.5 py-1 select-none"
      aria-label="Ovis — home"
    >
      <span className="font-display font-display-soft text-title font-semibold text-ink">
        Ovis
      </span>
      <span
        aria-hidden
        className="mb-2 size-1.5 rounded-full bg-linear-to-br from-gold to-mint"
      />
    </Link>
  );
}

/**
 * Sticky glass top bar: wordmark · search pill (all viewports — the F4 fix)
 * · health + refresh.
 */
export function TopBar({ onOpenMobileSearch }: { onOpenMobileSearch: () => void }) {
  const queryClient = useQueryClient();
  const fetching = useIsFetching();

  return (
    <header
      className="glass-pill sticky top-0 z-30 flex h-14 shrink-0 items-center gap-2 border-x-0 border-t-0 px-3 md:px-4"
      style={{ paddingTop: 'env(safe-area-inset-top)' }}
    >
      <div className="shrink-0">
        <Wordmark />
      </div>

      <div className="flex min-w-0 flex-1 justify-center px-1">
        <SearchPill onOpenMobile={onOpenMobileSearch} />
      </div>

      <div className="flex shrink-0 items-center gap-1">
        <IconButton
          label="Refresh data"
          onClick={() => queryClient.invalidateQueries()}
        >
          <RefreshCw className={cn('size-4', fetching > 0 && 'animate-spin')} aria-hidden />
        </IconButton>
        <HealthDot />
      </div>
    </header>
  );
}
