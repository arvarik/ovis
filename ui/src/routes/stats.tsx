import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from './__root';

/** Lazy: recharts stays out of the initial bundle. */
export const statsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'stats',
  component: lazyRouteComponent(() => import('@/components/stats/StatsView'), 'StatsView'),
});
