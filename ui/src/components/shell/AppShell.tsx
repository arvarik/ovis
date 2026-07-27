import { useState } from 'react';
import { Outlet, useNavigate } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { useHotkeys } from '@/hooks/hotkeys';
import { focusSearch } from '@/lib/searchFocus';
import { Toaster } from '@/components/primitives/Toaster';
import { MobileSearchSheet } from '@/components/documents/MobileSearchSheet';
import { TopBar } from './TopBar';
import { NavRail } from './NavRail';
import { BottomTabs } from './BottomTabs';
import { CommandPalette } from './CommandPalette';
import { HelpOverlay } from './HelpOverlay';

export function AppShell() {
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [searchSheetOpen, setSearchSheetOpen] = useState(false);
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  useHotkeys([
    {
      keys: 'mod+k',
      description: 'Open the command palette',
      group: 'Global',
      allowInInput: true,
      worksInOverlay: true,
      handler: () => setPaletteOpen(true),
    },
    {
      keys: '?',
      description: 'Keyboard shortcuts',
      group: 'Global',
      handler: () => setHelpOpen(true),
    },
    {
      keys: '/',
      description: 'Focus search',
      group: 'Global',
      handler: () => {
        if (!focusSearch()) setPaletteOpen(true);
      },
    },
    {
      keys: 'g p',
      description: 'Go to Pages',
      group: 'Navigation',
      handler: () => navigate({ to: '/pages' }),
    },
    {
      keys: 'g c',
      description: 'Go to Connectors',
      group: 'Navigation',
      handler: () => navigate({ to: '/connectors' }),
    },
    {
      keys: 'g a',
      description: 'Go to Activity',
      group: 'Navigation',
      handler: () => navigate({ to: '/activity' }),
    },
    {
      keys: 'g s',
      description: 'Go to Stats',
      group: 'Navigation',
      handler: () => navigate({ to: '/stats' }),
    },
    {
      keys: 'r',
      description: 'Refresh data',
      group: 'Global',
      handler: () => queryClient.invalidateQueries(),
    },
  ]);

  return (
    <div className="relative flex h-dvh flex-col overflow-hidden bg-canvas bg-aurora">
      <div aria-hidden className="bg-noise pointer-events-none absolute inset-0" />
      <TopBar onOpenMobileSearch={() => setSearchSheetOpen(true)} />
      <div className="flex min-h-0 flex-1">
        <NavRail />
        <main className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          <Outlet />
        </main>
      </div>
      <BottomTabs onOpenSearch={() => setPaletteOpen(true)} />
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
      <HelpOverlay open={helpOpen} onOpenChange={setHelpOpen} />
      <MobileSearchSheet open={searchSheetOpen} onOpenChange={setSearchSheetOpen} />
      <Toaster />
    </div>
  );
}
