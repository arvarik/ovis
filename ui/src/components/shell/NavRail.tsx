import { useState } from 'react';
import { Link, useRouterState } from '@tanstack/react-router';
import { Activity, BarChart3, Bot, Cable, FileText, Pin, Scissors } from 'lucide-react';
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
  { to: '/prune', label: 'Prune', icon: Scissors },
  { to: '/stats', label: 'Stats', icon: BarChart3 },
  { to: '/models', label: 'Models', icon: Bot },
] as const;

/**
 * Desktop-only (≥lg) icon rail: 64px, expanding to 256px on hover (overlay)
 * or pinned via ⌘. (in flow). One stable element tree — expansion is a width
 * transition on a single panel, never a structural swap, so nothing remounts
 * or flickers. The rail carries its own view-transition-name, so route
 * changes never repaint it.
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
        'relative z-20 hidden h-full shrink-0 lg:block [view-transition-name:nav-rail]',
        'transition-[width] duration-200 ease-swift',
        pinned ? 'w-64' : 'w-16',
      )}
    >
      <div
        className={cn(
          'absolute inset-y-0 left-0 flex flex-col overflow-hidden border-r py-3',
          'transition-[width,background-color,border-color,box-shadow] duration-200 ease-swift',
          expanded ? 'w-64' : 'w-16',
          hovered && !pinned
            ? 'border-line-2 bg-surface shadow-2xl shadow-black/40'
            : 'border-line bg-surface/60 shadow-none',
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
                    'relative flex h-10 items-center rounded-lg text-label transition-colors duration-150',
                    active ? 'bg-active text-ink' : 'text-ink-mute hover:bg-hover hover:text-ink',
                  )}
                >
                  {active ? (
                    <span
                      aria-hidden
                      className="absolute inset-y-2 left-0 w-0.5 rounded-full bg-gold"
                    />
                  ) : null}
                  {/* Fixed 44px icon column: icons never move as the width animates. */}
                  <span className="flex w-11 shrink-0 items-center justify-center">
                    <Icon className="size-4.5" aria-hidden />
                  </span>
                  <span
                    className={cn(
                      'truncate whitespace-nowrap transition-opacity duration-150',
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
          <div
            className={cn(
              'overflow-hidden px-3 font-mono text-caption whitespace-nowrap text-ink-faint transition-opacity duration-150',
              expanded ? 'opacity-100' : 'opacity-0',
            )}
          >
            {overview.data
              ? `${overview.data.documents_exact ? '' : '~'}${compact(overview.data.documents)} documents`
              : ' '}
          </div>
          <button
            type="button"
            onClick={togglePin}
            aria-pressed={pinned}
            title={pinned ? 'Unpin rail (⌘.)' : 'Pin rail (⌘.)'}
            className={cn(
              'flex h-9 items-center rounded-lg text-label transition-colors duration-150',
              pinned ? 'text-gold' : 'text-ink-faint hover:bg-hover hover:text-ink-mute',
            )}
          >
            <span className="flex w-11 shrink-0 items-center justify-center">
              <Pin className="size-4" aria-hidden />
            </span>
            <span
              className={cn(
                'truncate whitespace-nowrap transition-opacity duration-150',
                expanded ? 'opacity-100' : 'opacity-0',
              )}
            >
              {pinned ? 'Unpin rail' : 'Pin rail'}
              <span className="ml-2 font-mono text-caption text-ink-faint">⌘.</span>
            </span>
          </button>
        </div>
      </div>
    </nav>
  );
}
