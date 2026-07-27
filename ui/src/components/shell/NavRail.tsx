import { useState } from 'react';
import { Link, useRouterState } from '@tanstack/react-router';
import { Activity, BarChart3, Cable, FileText, Pin } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { overviewQuery } from '@/api/queries';
import { cn } from '@/lib/cn';
import { compact } from '@/lib/format';
import { useHotkeys } from '@/hooks/hotkeys';

const PIN_KEY = 'ovis:nav-pinned';

export const NAV_ENTRIES = [
  { to: '/pages', label: 'Pages', icon: FileText },
  { to: '/connectors', label: 'Connectors', icon: Cable },
  { to: '/activity', label: 'Activity', icon: Activity },
  { to: '/stats', label: 'Stats', icon: BarChart3 },
] as const;

/**
 * Desktop-only (≥lg) icon rail, 64px collapsed, expanding to 256px on
 * hover or pinned (⌘.). Expansion overlays the content — no reflow.
 */
export function NavRail() {
  const [pinned, setPinned] = useState(() => {
    try {
      return localStorage.getItem(PIN_KEY) === '1';
    } catch {
      return false;
    }
  });
  const [hovered, setHovered] = useState(false);
  const expanded = pinned || hovered;
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const overview = useQuery(overviewQuery);

  const togglePin = () => {
    setPinned((p) => {
      try {
        localStorage.setItem(PIN_KEY, p ? '0' : '1');
      } catch {
        // fine — pin just won't persist
      }
      return !p;
    });
  };

  useHotkeys([
    {
      keys: 'mod+.',
      description: 'Pin / unpin the navigation rail',
      group: 'Navigation',
      allowInInput: true,
      handler: togglePin,
    },
  ]);

  return (
    <nav
      aria-label="Primary"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className={cn(
        'relative z-20 hidden lg:flex h-full shrink-0 flex-col border-r border-line bg-surface/60 transition-[width] duration-200 ease-swift',
        pinned ? 'w-64' : 'w-16',
      )}
    >
      <div
        className={cn(
          'flex h-full flex-col overflow-hidden py-3',
          !pinned && expanded && 'absolute inset-y-0 left-0 w-64 border-r border-line-2 bg-surface shadow-2xl shadow-black/40',
          !pinned && !expanded && 'w-16',
        )}
      >
        <ul className="flex flex-col gap-1 px-2.5">
          {NAV_ENTRIES.map(({ to, label, icon: Icon }) => {
            const active = pathname.startsWith(to);
            return (
              <li key={to}>
                <Link
                  to={to}
                  className={cn(
                    'relative flex h-10 items-center gap-3 rounded-lg px-2.5 text-label transition-colors',
                    active
                      ? 'bg-active text-ink'
                      : 'text-ink-mute hover:bg-hover hover:text-ink',
                  )}
                >
                  {active ? (
                    <span
                      aria-hidden
                      className="absolute inset-y-2 left-0 w-0.5 rounded-full bg-gold"
                    />
                  ) : null}
                  <Icon className="size-4.5 shrink-0" aria-hidden />
                  <span
                    className={cn(
                      'truncate transition-opacity',
                      expanded ? 'opacity-100' : 'opacity-0',
                    )}
                  >
                    {label}
                  </span>
                </Link>
              </li>
            );
          })}
        </ul>

        <div className="mt-auto flex flex-col gap-1 px-2.5">
          {overview.data ? (
            <div
              className={cn(
                'px-2.5 font-mono text-caption text-ink-faint transition-opacity',
                expanded ? 'opacity-100' : 'opacity-0',
              )}
            >
              {overview.data.documents_exact ? '' : '~'}
              {compact(overview.data.documents)} documents
            </div>
          ) : null}
          <button
            type="button"
            onClick={togglePin}
            aria-pressed={pinned}
            className={cn(
              'flex h-9 items-center gap-3 rounded-lg px-2.5 text-label transition-colors',
              pinned ? 'text-gold' : 'text-ink-faint hover:bg-hover hover:text-ink-mute',
            )}
          >
            <Pin className="size-4 shrink-0" aria-hidden />
            <span className={cn('truncate transition-opacity', expanded ? 'opacity-100' : 'opacity-0')}>
              {pinned ? 'Unpin rail' : 'Pin rail'}
              <span className="ml-2 font-mono text-caption text-ink-faint">⌘.</span>
            </span>
          </button>
        </div>
      </div>
    </nav>
  );
}
