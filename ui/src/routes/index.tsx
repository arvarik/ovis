import { createRoute, redirect } from '@tanstack/react-router';
import { rootRoute } from './__root';

/** `/` → the Explorer. */
export const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  beforeLoad: () => {
    throw redirect({ to: '/pages' });
  },
});
