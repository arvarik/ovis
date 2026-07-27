import { createRouter } from '@tanstack/react-router';
import { rootRoute } from './routes/__root';
import { indexRoute } from './routes/index';
import { pagesRoute } from './routes/pages/index';
import { pageDetailRoute } from './routes/pages/docId';
import { connectorsRoute } from './routes/connectors/index';
import { connectorDetailRoute } from './routes/connectors/ccPairId';
import { activityRoute } from './routes/activity';
import { statsRoute } from './routes/stats';
import { labRoute } from './routes/lab';

const routeTree = rootRoute.addChildren([
  indexRoute,
  pagesRoute.addChildren([pageDetailRoute]),
  connectorsRoute,
  connectorDetailRoute,
  activityRoute,
  statsRoute,
  labRoute,
]);

export const router = createRouter({
  routeTree,
  defaultPreload: 'intent',
  // Document ids are URLs: never try to be clever about their characters.
  defaultPendingMinMs: 0,
  scrollRestoration: true,
  // Route changes morph via the View Transitions API where available;
  // theme.css disables the pseudo-element animations under reduced motion.
  defaultViewTransition: true,
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
