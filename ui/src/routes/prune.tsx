import { createRoute, lazyRouteComponent } from '@tanstack/react-router';
import { rootRoute } from './__root';

const TABS = ['triage', 'review', 'clusters', 'staged', 'trash', 'rules', 'history'] as const;
export type PruneTab = (typeof TABS)[number];

export interface PruneSearch {
  tab: PruneTab;
  /** Set when Triage hands a bundle off to the filtered review list. */
  detector?: string;
}

export const pruneRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'prune',
  validateSearch: (search: Record<string, unknown>): PruneSearch => {
    const tab = typeof search.tab === 'string' ? search.tab : '';
    const detector = typeof search.detector === 'string' ? search.detector : undefined;
    return {
      // Triage is the landing tab: the backlog is six figures long, and a flat
      // list is not where anyone should start reading it.
      tab: TABS.includes(tab as PruneTab) ? (tab as PruneTab) : 'triage',
      detector,
    };
  },
  component: lazyRouteComponent(() => import('@/components/prune/PruneView'), 'PruneView'),
});
