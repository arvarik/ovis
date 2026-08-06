import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from './__root';

export const modelsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'models',
  component: lazyRouteComponent(() => import('@/components/models/ModelsView'), 'ModelsView'),
});
