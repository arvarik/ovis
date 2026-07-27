import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';

// Self-hosted fonts — this app must render on a LAN with no internet.
import '@fontsource-variable/inter';
import '@fontsource-variable/fraunces/full.css';
import '@fontsource-variable/jetbrains-mono';
import './styles/theme.css';

import { router } from './router';
import { TooltipProvider } from './components/primitives/Tooltip';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 15_000,
      retry: 1,
      refetchOnWindowFocus: true,
    },
  },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={350}>
        <RouterProvider router={router} />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
);
