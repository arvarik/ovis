import { useState, useEffect, useCallback } from 'react';
import { PageListItem } from '../api/types';
import { fetchPages, deletePage, batchDeletePages } from '../api/client';
import { subscribeToPagesStream } from '../api/sse';

export function usePages() {
  const [pages, setPages] = useState<PageListItem[]>([]);
  const [total, setTotal] = useState<number>(0);
  const [page, setPage] = useState<number>(1);
  const [limit, setLimitState] = useState<number>(50);

  const setLimit = (newLimit: number) => {
    setLimitState(newLimit);
    setPage(1);
  };
  const [search, setSearch] = useState<string>('');
  const [selectedConnector, setSelectedConnector] = useState<number | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [useSSE, setUseSSE] = useState<boolean>(false);
  const [streamStats, setStreamStats] = useState<{ time_ms: number } | null>(null);

  const loadPages = useCallback(async () => {
    setLoading(true);
    setError(null);

    if (useSSE) {
      setPages([]);
      subscribeToPagesStream({
        search,
        connector_id: selectedConnector,
        limit,
        onPage: (newPage) => {
          setPages((prev) => {
            if (prev.some((p) => p.id === newPage.id)) return prev;
            return [...prev, newPage];
          });
        },
        onDone: (summary) => {
          setTotal(summary.total_matched);
          setStreamStats({ time_ms: summary.time_ms });
          setLoading(false);
        },
        onError: () => {
          // Fall back to REST if SSE fails
          setUseSSE(false);
        },
      });
    } else {
      try {
        const res = await fetchPages({
          page,
          limit,
          search,
          connector_id: selectedConnector,
        });
        setPages(res.items);
        setTotal(res.total);
      } catch (err: any) {
        console.error('Failed to fetch pages from backend:', err.message);
        setPages([]);
        setTotal(0);
        setError(err.message || 'Failed to load document pages');
      } finally {
        setLoading(false);
      }
    }
  }, [page, limit, search, selectedConnector, useSSE]);

  useEffect(() => {
    loadPages();
  }, [loadPages]);

  const removePage = async (id: string) => {
    try {
      await deletePage(id);
    } catch {
      // Local optimistic deletion
    }
    setPages((prev) => prev.filter((p) => p.id !== id));
    setTotal((prev) => Math.max(0, prev - 1));
  };

  const removeBatch = async (ids: string[]) => {
    try {
      await batchDeletePages(ids);
    } catch {
      // Local optimistic deletion
    }
    setPages((prev) => prev.filter((p) => !ids.includes(p.id)));
    setTotal((prev) => Math.max(0, prev - ids.length));
  };

  return {
    pages,
    total,
    page,
    setPage,
    limit,
    setLimit,
    search,
    setSearch,
    selectedConnector,
    setSelectedConnector,
    loading,
    error,
    useSSE,
    setUseSSE,
    streamStats,
    refetch: loadPages,
    removePage,
    removeBatch,
  };
}
