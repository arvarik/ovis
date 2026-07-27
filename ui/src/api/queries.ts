/**
 * queryOptions factories — the one place query keys, fetchers and cache
 * policies live. Defaults per 02_ARCHITECTURE_AND_STACK.md §3: 15 s lists,
 * 60 s detail, 5 s health/activity.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, encodeDocId, type QueryParams } from './client';
import type {
  ChunksResponse,
  PruneAuditItem,
  PruneCandidateDetail,
  PruneCandidateItem,
  PruneRuleItem,
  PruneScanItem,
  PruneStatusResponse,
  ChunkVector,
  ConnectorDetail,
  ConnectorSummary,
  IndexAttemptErrorsResponse,
  IndexAttemptItem,
  ListResponse,
  PageDetail,
  PageListItem,
  RuntimeResponse,
  SearchResponse,
  SourceStat,
  StatsOverview,
  TagFacet,
  TimelineResponse,
  TopConnector,
} from './types';
import type { PagesSearch } from '@/routes/pages';

export const healthQuery = queryOptions({
  queryKey: ['system', 'health'],
  queryFn: ({ signal }) => api.health(signal),
  staleTime: 5_000,
  refetchInterval: 30_000,
  // A degraded backend is data here, not an exception to retry into silence.
  retry: 1,
});

export const runtimeQuery = queryOptions({
  queryKey: ['system', 'runtime'],
  queryFn: ({ signal }) => api.get<RuntimeResponse>('/system/runtime', undefined, signal),
  staleTime: 60_000,
});

export const overviewQuery = queryOptions({
  queryKey: ['stats', 'overview'],
  queryFn: ({ signal }) => api.get<StatsOverview>('/stats/overview', undefined, signal),
  staleTime: 15_000,
});

export function timelineQuery(window: '24h' | '7d' | '30d') {
  return queryOptions({
    queryKey: ['stats', 'timeline', window],
    queryFn: ({ signal }) =>
      api.get<TimelineResponse>(
        '/stats/timeline',
        { window, bucket: window === '24h' ? '1h' : '1d' },
        signal,
      ),
    staleTime: 60_000,
  });
}

export const sourcesQuery = queryOptions({
  queryKey: ['stats', 'sources'],
  queryFn: ({ signal }) => api.get<SourceStat[]>('/stats/sources', undefined, signal),
  staleTime: 60_000,
});

export function topConnectorsQuery(by: 'docs' | 'recent' = 'docs', limit = 10) {
  return queryOptions({
    queryKey: ['stats', 'top-connectors', by, limit],
    queryFn: ({ signal }) =>
      api.get<TopConnector[]>('/stats/connectors/top', { by, limit }, signal),
    staleTime: 60_000,
  });
}

/** Tag facet counts (server-cached 60 s). */
export function tagsQuery(limit = 10) {
  return queryOptions({
    queryKey: ['tags', limit],
    queryFn: ({ signal }) => api.get<TagFacet[]>('/tags', { limit }, signal),
    staleTime: 60_000,
  });
}

/** `GET /connectors` answers a bare array (cli/05_AS_BUILT.md §2.4). */
export const connectorsQuery = queryOptions({
  queryKey: ['connectors'],
  queryFn: ({ signal }) => api.get<ConnectorSummary[]>('/connectors', undefined, signal),
  staleTime: 15_000,
});

// ---------------------------------------------------------------------------
// Pages list & content search
// ---------------------------------------------------------------------------

export const PAGE_LIMIT = 100;

/** URL state -> `GET /pages` params. Unknown params 400, so map explicitly. */
export function buildListParams(search: PagesSearch): QueryParams {
  return {
    search: search.search,
    connector_id: search.connector,
    source: search.source,
    hidden: search.hidden,
    chunk_min: search.chunk_min,
    chunk_max: search.chunk_max,
    updated_after: search.updated_after,
    updated_before: search.updated_before,
    sort: search.sort,
  };
}

export function pagesInfiniteQuery(search: PagesSearch) {
  const params = buildListParams(search);
  return infiniteQueryOptions({
    queryKey: ['pages', 'list', params],
    queryFn: ({ pageParam, signal }) =>
      api.get<ListResponse<PageListItem>>(
        '/pages',
        { ...params, limit: PAGE_LIMIT, ...(pageParam ? { cursor: pageParam } : {}) },
        signal,
      ),
    initialPageParam: null as string | null,
    // Keyset cursor — no depth limit, unlike offset paging (refused past 50k).
    getNextPageParam: (last) => (last.has_more ? last.next_cursor : null),
    staleTime: 15_000,
  });
}

/**
 * `GET /search` — the param is `mode`, NOT `search_mode` (the API validates
 * strictly). The response echoes the requested mode and reports any fallback
 * in `degraded`; key off `degraded`, never off `mode`.
 */
export function searchQuery(search: PagesSearch) {
  const q = search.q ?? '';
  return queryOptions({
    queryKey: ['search', q, search.mode ?? 'keyword', search.connector ?? null, search.source ?? null],
    queryFn: ({ signal }) =>
      api.get<SearchResponse>(
        '/search',
        {
          q,
          mode: search.mode,
          connector_id: search.connector,
          source: search.source,
          limit: 100,
        },
        signal,
      ),
    enabled: q !== '',
    staleTime: 15_000,
  });
}

// ---------------------------------------------------------------------------
// Page detail, chunks, text, vectors
// ---------------------------------------------------------------------------

export function pageDetailQuery(docId: string) {
  return queryOptions({
    queryKey: ['pages', 'detail', docId],
    queryFn: ({ signal }) =>
      api.get<PageDetail>(`/pages/${encodeDocId(docId)}`, undefined, signal),
    staleTime: 60_000,
  });
}

export function pageChunksQuery(docId: string) {
  return infiniteQueryOptions({
    queryKey: ['pages', 'chunks', docId],
    queryFn: ({ pageParam, signal }) =>
      api.get<ChunksResponse>(
        `/pages/${encodeDocId(docId)}/chunks`,
        { limit: 25, ...(pageParam !== null ? { after: pageParam } : {}) },
        signal,
      ),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.next_after,
    staleTime: 60_000,
  });
}

export function pageTextQuery(docId: string) {
  return queryOptions({
    queryKey: ['pages', 'text', docId],
    queryFn: ({ signal }) => api.getText(`/pages/${encodeDocId(docId)}/text`, undefined, signal),
    staleTime: 60_000,
  });
}

/** One REAL vector (D3 fix — the old UI fabricated 1,536 floats). */
export function chunkVectorQuery(docId: string, chunkIndex: number) {
  return queryOptions({
    queryKey: ['pages', 'vector', docId, chunkIndex],
    queryFn: ({ signal }) =>
      api.get<ChunkVector>(
        `/pages/${encodeDocId(docId)}/chunks/${chunkIndex}/vector`,
        undefined,
        signal,
      ),
    staleTime: Infinity,
  });
}

// ---------------------------------------------------------------------------
// Connectors & indexing activity
// ---------------------------------------------------------------------------

/** History (7-day sparkline) exists ONLY here — never on the fleet list. */
export function connectorDetailQuery(ccPairId: number) {
  return queryOptions({
    queryKey: ['connectors', 'detail', ccPairId],
    queryFn: ({ signal }) =>
      api.get<ConnectorDetail>(`/connectors/${ccPairId}`, { history: '7d' }, signal),
    staleTime: 15_000,
  });
}

/** Attempts paginate by page number (next_cursor is null on this endpoint). */
export function connectorAttemptsQuery(ccPairId: number) {
  return infiniteQueryOptions({
    queryKey: ['connectors', 'attempts', ccPairId],
    queryFn: ({ pageParam, signal }) =>
      api.get<ListResponse<IndexAttemptItem>>(
        `/connectors/${ccPairId}/attempts`,
        { limit: 25, page: pageParam },
        signal,
      ),
    initialPageParam: 1,
    getNextPageParam: (last) => (last.has_more && last.page !== null ? last.page + 1 : null),
    staleTime: 5_000,
  });
}

export function connectorErrorsQuery(ccPairId: number) {
  return infiniteQueryOptions({
    queryKey: ['connectors', 'errors', ccPairId],
    queryFn: ({ pageParam, signal }) =>
      api.get<IndexAttemptErrorsResponse>(
        `/connectors/${ccPairId}/errors`,
        { limit: 50, page: pageParam },
        signal,
      ),
    initialPageParam: 1,
    getNextPageParam: (last) => (last.has_more && last.page !== null ? last.page + 1 : null),
    staleTime: 5_000,
  });
}

/** The pair's authoritative doc list (dcc join — includes multi-connector docs). */
export function connectorDocsQuery(ccPairId: number) {
  return infiniteQueryOptions({
    queryKey: ['connectors', 'docs', ccPairId],
    queryFn: ({ pageParam, signal }) =>
      api.get<ListResponse<PageListItem>>(
        `/connectors/${ccPairId}/docs`,
        { limit: PAGE_LIMIT, ...(pageParam ? { cursor: pageParam } : {}) },
        signal,
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => (last.has_more ? last.next_cursor : null),
    staleTime: 15_000,
  });
}

export function indexingAttemptsQuery(status?: string, limit = 50) {
  return queryOptions({
    queryKey: ['indexing', 'attempts', status ?? 'all', limit],
    queryFn: ({ signal }) =>
      api.get<ListResponse<IndexAttemptItem>>('/indexing/attempts', { status, limit }, signal),
    staleTime: 4_000,
  });
}

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

/** The status strip's source of truth. Server numbers, never client-derived. */
export const pruneStatusQuery = queryOptions({
  queryKey: ['prune', 'status'],
  queryFn: ({ signal }) =>
    api.get<PruneStatusResponse>('/prune/status', undefined, signal),
  staleTime: 4_000,
  refetchInterval: 5_000,
});

export function pruneCandidatesQuery(params: QueryParams) {
  return queryOptions({
    queryKey: ['prune', 'candidates', params],
    queryFn: ({ signal }) =>
      api.get<ListResponse<PruneCandidateItem>>(
        '/prune/candidates',
        params,
        signal,
      ),
    staleTime: 10_000,
  });
}

export function pruneCandidateQuery(id: number) {
  return queryOptions({
    queryKey: ['prune', 'candidate', id],
    queryFn: ({ signal }) =>
      api.get<PruneCandidateDetail>(`/prune/candidates/${id}`, undefined, signal),
    staleTime: 10_000,
  });
}

export function pruneScansQuery(limit = 20) {
  return queryOptions({
    queryKey: ['prune', 'scans', limit],
    queryFn: ({ signal }) =>
      api.get<ListResponse<PruneScanItem>>('/prune/scans', { limit }, signal),
    staleTime: 5_000,
  });
}

/** Poll target while a scan runs; the component keys refetch off status. */
export function pruneScanQuery(id: number) {
  return queryOptions({
    queryKey: ['prune', 'scan', id],
    queryFn: ({ signal }) =>
      api.get<PruneScanItem>(`/prune/scans/${id}`, undefined, signal),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status === 'queued' || status === 'running' ? 1_500 : false;
    },
  });
}

export const pruneRulesQuery = queryOptions({
  queryKey: ['prune', 'rules'],
  queryFn: ({ signal }) => api.get<PruneRuleItem[]>('/prune/rules', undefined, signal),
  staleTime: 15_000,
});

export function pruneAuditQuery(params: QueryParams) {
  return queryOptions({
    queryKey: ['prune', 'audit', params],
    queryFn: ({ signal }) =>
      api.get<ListResponse<PruneAuditItem>>('/prune/audit', params, signal),
    staleTime: 5_000,
  });
}

export const pruneConfigQuery = queryOptions({
  queryKey: ['prune', 'config'],
  queryFn: ({ signal }) => api.getText('/prune/config', undefined, signal),
  staleTime: 15_000,
});

/**
 * Global preset counts (D5 fix: counts are global truths, or absent).
 * One `limit=1` list request per preset; ~1 ms each server-side.
 */
export function presetCountQuery(params: QueryParams) {
  return queryOptions({
    queryKey: ['pages', 'count', params],
    queryFn: async ({ signal }) => {
      const res = await api.get<ListResponse<PageListItem>>(
        '/pages',
        { ...params, limit: 1 },
        signal,
      );
      return { total: res.total, exact: res.total_exact };
    },
    staleTime: 60_000,
  });
}
