import { createRoute, Outlet } from '@tanstack/react-router';
import { rootRoute } from '../__root';
import { PagesView } from '@/components/documents/PagesView';

export const PAGE_SORTS = [
  'updated_desc',
  'updated_asc',
  'chunks_desc',
  'chunks_asc',
  'id_asc',
  'id_desc',
  'boost_desc',
] as const;
export type PagesSort = (typeof PAGE_SORTS)[number];

export const SEARCH_MODES = ['keyword', 'semantic', 'hybrid'] as const;

/**
 * URL is the state (F2 fix): every filter, the query and the sort live here.
 * `q` switches the view to content search (`GET /search`, param `mode` — NOT
 * `search_mode`, the API rejects unknown params with 400); `search` is the
 * list endpoint's title/id substring filter.
 */
export interface PagesSearch {
  q?: string;
  mode?: (typeof SEARCH_MODES)[number];
  search?: string;
  connector?: number;
  source?: string;
  hidden?: boolean;
  chunk_min?: number;
  chunk_max?: number;
  updated_after?: string;
  updated_before?: string;
  sort?: PagesSort;
  live?: boolean;
}

function optStr(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}

function optNum(v: unknown): number | undefined {
  const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v) : NaN;
  return Number.isFinite(n) ? n : undefined;
}

function optBool(v: unknown): boolean | undefined {
  if (typeof v === 'boolean') return v;
  if (v === 'true') return true;
  if (v === 'false') return false;
  return undefined;
}

export const pagesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'pages',
  validateSearch: (search: Record<string, unknown>): PagesSearch => {
    const sort = optStr(search.sort);
    const mode = optStr(search.mode);
    return {
      q: optStr(search.q),
      mode: SEARCH_MODES.includes(mode as never) ? (mode as PagesSearch['mode']) : undefined,
      search: optStr(search.search),
      connector: optNum(search.connector),
      source: optStr(search.source),
      hidden: optBool(search.hidden),
      chunk_min: optNum(search.chunk_min),
      chunk_max: optNum(search.chunk_max),
      updated_after: optStr(search.updated_after),
      updated_before: optStr(search.updated_before),
      sort: PAGE_SORTS.includes(sort as never) ? (sort as PagesSort) : undefined,
      live: optBool(search.live),
    };
  },
  component: PagesLayout,
});

function PagesLayout() {
  return (
    <>
      <PagesView />
      {/* The $docId inspector renders over the list. */}
      <Outlet />
    </>
  );
}
