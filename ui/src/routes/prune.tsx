import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from './__root';

const TABS = ['review', 'staged', 'rules', 'history'] as const;
export type PruneTab = (typeof TABS)[number];

export interface PruneSearch {
  tab: PruneTab;
}

export const pruneRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'prune',
  validateSearch: (search: Record<string, unknown>): PruneSearch => {
    const tab = typeof search.tab === 'string' ? search.tab : '';
    return { tab: TABS.includes(tab as PruneTab) ? (tab as PruneTab) : 'review' };
  },
  component: lazyRouteComponent(() => import('@/components/prune/PruneView'), 'PruneView'),
});
