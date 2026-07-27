import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from '../__root';

export const CONNECTOR_SORTS = ['docs', 'recent', 'errors', 'name'] as const;
export type ConnectorsSort = (typeof CONNECTOR_SORTS)[number];

export interface ConnectorsSearch {
  status?: string;
  source?: string;
  filter?: string;
  sort?: ConnectorsSort;
}

export const connectorsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'connectors',
  validateSearch: (search: Record<string, unknown>): ConnectorsSearch => {
    const sort = typeof search.sort === 'string' ? search.sort : undefined;
    return {
      status: typeof search.status === 'string' && search.status !== '' ? search.status : undefined,
      source: typeof search.source === 'string' && search.source !== '' ? search.source : undefined,
      filter: typeof search.filter === 'string' && search.filter !== '' ? search.filter : undefined,
      sort: CONNECTOR_SORTS.includes(sort as never) ? (sort as ConnectorsSort) : undefined,
    };
  },
  component: lazyRouteComponent(
    () => import('@/components/connectors/ConnectorsView'),
    'ConnectorsView',
  ),
});
