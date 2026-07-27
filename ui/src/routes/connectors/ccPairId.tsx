import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from '../__root';

export const CONNECTOR_TABS = ['attempts', 'errors', 'documents'] as const;
export type ConnectorTab = (typeof CONNECTOR_TABS)[number];

export interface ConnectorDetailSearch {
  tab?: ConnectorTab;
}

export const connectorDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'connectors/$ccPairId',
  parseParams: ({ ccPairId }) => ({ ccPairId: Number(ccPairId) }),
  stringifyParams: ({ ccPairId }) => ({ ccPairId: String(ccPairId) }),
  validateSearch: (search: Record<string, unknown>): ConnectorDetailSearch => {
    const tab = typeof search.tab === 'string' ? search.tab : undefined;
    return {
      tab: CONNECTOR_TABS.includes(tab as never) ? (tab as ConnectorTab) : undefined,
    };
  },
  component: lazyRouteComponent(
    () => import('@/components/connectors/ConnectorDetailView'),
    'ConnectorDetailView',
  ),
});
