import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { pagesRoute } from './index';

/**
 * `/pages/$docId` — the document id is a URL occupying exactly one
 * percent-encoded path segment. The router must round-trip ids containing
 * `/`, `?` and `#` without mangling them; `params.docId` arrives decoded.
 * Lazy: react-markdown and the viewers stay out of the initial bundle.
 */
export const pageDetailRoute = createRoute({
  getParentRoute: () => pagesRoute,
  path: '$docId',
  component: lazyRouteComponent(() => import('@/components/documents/Inspector'), 'InspectorRoute'),
});
