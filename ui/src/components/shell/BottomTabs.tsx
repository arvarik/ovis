import { Link, useRouterState } from '@tanstack/react-router';
import { Search } from 'lucide-react';
import { cn } from '@/lib/cn';
import { NAV_ENTRIES } from './NavRail';

/**
 * Mobile navigation (<lg): the same 4 routes as the rail — one information
 * architecture, two projections — plus a center search button. 44px+ targets,
 * safe-area padded.
 */
export function BottomTabs({ onOpenSearch }: { onOpenSearch: () => void }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const [pages, connectors, activity, prune, stats] = NAV_ENTRIES;
  const left = [pages, connectors];
  const right = [activity, prune, stats];

  const renderTab = ({ to, label, icon: Icon }: (typeof NAV_ENTRIES)[number]) => {
    const active = pathname.startsWith(to);
    return (
      <Link
        key={to}
        to={to}
        className={cn(
          'flex min-h-11 flex-1 flex-col items-center justify-center gap-0.5 rounded-lg py-1.5 transition-colors',
          active ? 'text-gold' : 'text-ink-faint hover:text-ink-mute',
        )}
      >
        <Icon className="size-5" aria-hidden />
        <span className="text-caption leading-none font-medium">{label}</span>
      </Link>
    );
  };

  return (
    <nav
      aria-label="Primary"
      className="glass-pill z-30 flex shrink-0 items-stretch gap-1 border-x-0 border-b-0 px-2 pt-1.5 lg:hidden [view-transition-name:bottom-tabs]"
      style={{ paddingBottom: 'max(env(safe-area-inset-bottom), 0.375rem)' }}
    >
      {left.map(renderTab)}
      <button
        type="button"
        onClick={onOpenSearch}
        aria-label="Search"
        className="mx-1 -mt-4 flex size-12 shrink-0 items-center justify-center self-start rounded-full bg-gold text-canvas shadow-lg shadow-black/40 transition-colors active:bg-gold-bright"
      >
        <Search className="size-5" aria-hidden />
      </button>
      {right.map(renderTab)}
    </nav>
  );
}
