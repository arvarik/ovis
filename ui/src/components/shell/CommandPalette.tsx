import { useState } from 'react';
import { Command } from 'cmdk';
import { Dialog as RadixDialog } from 'radix-ui';
import { useNavigate } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Activity,
  BarChart3,
  Cable,
  FileClock,
  FileText,
  RefreshCw,
  SearchIcon,
} from 'lucide-react';
import { connectorsQuery } from '@/api/queries';
import { cn } from '@/lib/cn';
import { compact, sourceLabel } from '@/lib/format';
import { getRecentDocs } from '@/lib/recentDocs';
import { useHotkeyLayer } from '@/hooks/hotkeys';
import { statusTone } from '@/components/primitives/Badge';
import { Kbd } from '@/components/primitives/Kbd';

const TONE_DOT: Record<string, string> = {
  mint: 'bg-mint',
  gold: 'bg-gold',
  rose: 'bg-rose',
  indigo: 'bg-indigo',
  violet: 'bg-violet',
  teal: 'bg-teal',
  neutral: 'bg-ink-faint',
};

function PaletteItem({
  onSelect,
  icon,
  children,
  hint,
  value,
  keywords,
  forceMount,
}: {
  onSelect: () => void;
  icon?: React.ReactNode;
  children: React.ReactNode;
  hint?: string;
  value?: string;
  keywords?: string[];
  forceMount?: true;
}) {
  return (
    <Command.Item
      value={value}
      keywords={keywords}
      forceMount={forceMount}
      onSelect={onSelect}
      className={cn(
        'flex min-h-11 cursor-default items-center gap-3 rounded-lg px-3 text-body md:min-h-9 md:text-label',
        'text-ink-mute select-none',
        // Gold is the one selection color (fixes the old rose inconsistency).
        'data-[selected=true]:bg-active data-[selected=true]:text-ink data-[selected=true]:shadow-[inset_2px_0_0_var(--color-gold)]',
      )}
    >
      {icon ? <span className="text-ink-faint [&>svg]:size-4">{icon}</span> : null}
      <span className="flex-1 truncate">{children}</span>
      {hint ? <span className="font-mono text-caption text-ink-faint">{hint}</span> : null}
    </Command.Item>
  );
}

export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [query, setQuery] = useState('');
  const connectors = useQuery({ ...connectorsQuery, enabled: open });
  useHotkeyLayer('palette', open);

  const handleOpenChange = (o: boolean) => {
    if (!o) setQuery('');
    onOpenChange(o);
  };

  const run = (fn: () => void) => {
    handleOpenChange(false);
    fn();
  };

  const recents = open ? getRecentDocs() : [];
  // Without a query, surface the biggest connectors; cmdk filters the rest in.
  const connectorItems = (connectors.data ?? [])
    .slice()
    .sort((a, b) => b.doc_count - a.doc_count)
    .slice(0, query ? undefined : 8);

  return (
    <RadixDialog.Root open={open} onOpenChange={handleOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-50 bg-black/60 animate-fade-in" />
        <RadixDialog.Content
          aria-describedby={undefined}
          className={cn(
            'fixed z-50 flex flex-col outline-none',
            // Mobile: full-screen sheet, input at top, keyboard-safe height.
            'inset-0 bg-canvas',
            // Desktop: centered glass panel.
            'md:inset-x-0 md:top-24 md:bottom-auto md:mx-auto md:h-auto md:max-h-[60dvh] md:w-full md:max-w-xl md:rounded-2xl md:glass-panel md:animate-scale-in',
          )}
        >
          <RadixDialog.Title className="sr-only">Command palette</RadixDialog.Title>
          <Command loop className="flex min-h-0 flex-1 flex-col">
            <div className="flex items-center gap-3 border-b border-line px-4">
              <SearchIcon className="size-4 shrink-0 text-ink-faint" aria-hidden />
              <Command.Input
                value={query}
                onValueChange={setQuery}
                autoFocus
                placeholder="Type a command or search…"
                className="h-14 w-full bg-transparent text-base text-ink outline-none placeholder:text-ink-faint md:h-12 md:text-body"
              />
              <button
                type="button"
                onClick={() => onOpenChange(false)}
                className="text-label text-ink-faint md:hidden"
              >
                Cancel
              </button>
            </div>

            <Command.List className="min-h-0 flex-1 overflow-y-auto p-2 pb-[max(env(safe-area-inset-bottom),0.5rem)]">
              <Command.Empty className="px-3 py-8 text-center text-label text-ink-faint">
                Nothing matches.
              </Command.Empty>

              <Command.Group
                heading="Actions"
                className="[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-caption [&_[cmdk-group-heading]]:text-ink-faint"
              >
                <PaletteItem
                  icon={<FileText />}
                  hint="g p"
                  onSelect={() => run(() => navigate({ to: '/pages' }))}
                >
                  Go to Pages
                </PaletteItem>
                <PaletteItem
                  icon={<Cable />}
                  hint="g c"
                  onSelect={() => run(() => navigate({ to: '/connectors' }))}
                >
                  Go to Connectors
                </PaletteItem>
                <PaletteItem
                  icon={<Activity />}
                  hint="g a"
                  onSelect={() => run(() => navigate({ to: '/activity' }))}
                >
                  Go to Activity
                </PaletteItem>
                <PaletteItem
                  icon={<BarChart3 />}
                  hint="g s"
                  onSelect={() => run(() => navigate({ to: '/stats' }))}
                >
                  Go to Stats
                </PaletteItem>
                <PaletteItem
                  icon={<RefreshCw />}
                  hint="r"
                  onSelect={() => run(() => queryClient.invalidateQueries())}
                >
                  Refresh data
                </PaletteItem>
              </Command.Group>

              {connectorItems.length > 0 ? (
                <Command.Group
                  heading="Connectors"
                  className="[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-caption [&_[cmdk-group-heading]]:text-ink-faint"
                >
                  {connectorItems.map((c) => (
                    <PaletteItem
                      key={c.cc_pair_id}
                      value={`connector ${c.name}`}
                      keywords={[c.source, c.status]}
                      hint={`${compact(c.doc_count)} docs`}
                      icon={
                        <span
                          aria-hidden
                          className={cn(
                            'block size-2 rounded-full',
                            TONE_DOT[statusTone(c.status)],
                          )}
                        />
                      }
                      onSelect={() =>
                        run(() =>
                          navigate({
                            to: '/pages',
                            search: { connector: c.connector_id },
                          }),
                        )
                      }
                    >
                      {c.name}
                      <span className="ml-2 text-caption text-ink-faint">
                        {sourceLabel(c.source)}
                      </span>
                    </PaletteItem>
                  ))}
                </Command.Group>
              ) : null}

              {recents.length > 0 ? (
                <Command.Group
                  heading="Recent documents"
                  className="[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-caption [&_[cmdk-group-heading]]:text-ink-faint"
                >
                  {recents.map((d) => (
                    <PaletteItem
                      key={d.id}
                      value={`recent ${d.title} ${d.id}`}
                      icon={<FileClock />}
                      onSelect={() =>
                        run(() =>
                          navigate({ to: '/pages/$docId', params: { docId: d.id } }),
                        )
                      }
                    >
                      {d.title || d.id}
                    </PaletteItem>
                  ))}
                </Command.Group>
              ) : null}

              {query.trim() !== '' ? (
                <Command.Group
                  heading="Content search"
                  forceMount
                  className="[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-caption [&_[cmdk-group-heading]]:text-ink-faint"
                >
                  <PaletteItem
                    forceMount
                    value={`search-pages-for ${query}`}
                    icon={<SearchIcon />}
                    onSelect={() =>
                      run(() => navigate({ to: '/pages', search: { q: query.trim() } }))
                    }
                  >
                    Search pages for “{query.trim()}”
                  </PaletteItem>
                </Command.Group>
              ) : null}
            </Command.List>

            <div className="hidden items-center gap-3 border-t border-line px-4 py-2 text-caption text-ink-faint md:flex">
              <span className="flex items-center gap-1">
                <Kbd>↑</Kbd>
                <Kbd>↓</Kbd> navigate
              </span>
              <span className="flex items-center gap-1">
                <Kbd>↵</Kbd> select
              </span>
              <span className="flex items-center gap-1">
                <Kbd>Esc</Kbd> close
              </span>
            </div>
          </Command>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
