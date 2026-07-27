import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from './__root';

export interface ActivitySearch {
  status?: string;
}

export const activityRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'activity',
  validateSearch: (search: Record<string, unknown>): ActivitySearch => ({
    status: typeof search.status === 'string' && search.status !== '' ? search.status : undefined,
  }),
  component: lazyRouteComponent(
    () => import('@/components/connectors/ActivityView'),
    'ActivityView',
  ),
});
