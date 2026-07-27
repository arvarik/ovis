/**
 * Mutations are honest (D1/D2 fixes):
 * - Optimistic removal always carries a rollback; a failed server delete is a
 *   persistent error toast and the rows come back visibly.
 * - Outcome toasts report what actually happened — `chunks_deleted`,
 *   `index_cleanup_pending`, `recrawl_risk`, per-item `failed[]` — never a
 *   blanket "success".
 * - There is no fake undo. Documents are hard-deleted; the reversible
 *   alternative is hide/unhide via PATCH, offered in the delete dialog.
 */
import { useMutation, useQueryClient, type InfiniteData } from '@tanstack/react-query';
import { toast } from 'sonner';
import { api, encodeDocId, ApiError } from './client';
import type {
  ActionResponse,
  BatchDeleteResponse,
  ConnectorPatchRequest,
  DeleteOutcome,
  ListResponse,
  PageListItem,
  PagePatch,
  PatchResponse,
  RunOnceRequest,
} from './types';
import { count as formatCount } from '@/lib/format';

type PagesData = InfiniteData<ListResponse<PageListItem>>;

function removeIdsFromLists(
  queryClient: ReturnType<typeof useQueryClient>,
  ids: ReadonlySet<string>,
) {
  const snapshots: Array<[readonly unknown[], PagesData]> = [];
  for (const [key, data] of queryClient.getQueriesData<PagesData>({ queryKey: ['pages', 'list'] })) {
    if (!data) continue;
    snapshots.push([key, data]);
    queryClient.setQueryData<PagesData>(key, {
      ...data,
      pages: data.pages.map((p) => ({
        ...p,
        items: p.items.filter((item) => !ids.has(item.id)),
      })),
    });
  }
  return () => {
    for (const [key, data] of snapshots) queryClient.setQueryData(key, data);
  };
}

function errorToast(title: string, error: unknown) {
  const apiError = error instanceof ApiError ? error : null;
  toast.error(title, {
    description: apiError
      ? `${apiError.message} (${apiError.code}${apiError.reqId ? ` · req ${apiError.reqId}` : ''})`
      : error instanceof Error
        ? error.message
        : 'unknown error',
    duration: Infinity,
  });
}

function invalidatePages(queryClient: ReturnType<typeof useQueryClient>) {
  void queryClient.invalidateQueries({ queryKey: ['pages'] });
  void queryClient.invalidateQueries({ queryKey: ['stats'] });
  void queryClient.invalidateQueries({ queryKey: ['search'] });
}

export function usePatchPage(docId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (patch: PagePatch) =>
      api.patch<PatchResponse>(`/pages/${encodeDocId(docId)}`, patch),
    onSuccess: (res, patch) => {
      queryClient.setQueryData(['pages', 'detail', docId], res);
      invalidatePages(queryClient);
      const bits: string[] = [];
      if (patch.semantic_id !== undefined)
        bits.push(res.index_synced ? 'title synced to index' : 'title saved — index sync pending');
      if (patch.boost !== undefined || patch.hidden !== undefined)
        bits.push(
          res.boost_hidden_via === 'onyx_api'
            ? 'applied via Onyx API'
            : res.boost_hidden_via === 'direct_sql'
              ? 'applied directly (no Onyx key)'
              : '',
        );
      toast.success('Saved', { description: bits.filter(Boolean).join(' · ') || undefined });
    },
    onError: (error) => errorToast('Edit failed', error),
  });
}

export function useDeletePage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (docId: string) =>
      api.delete<DeleteOutcome>(`/pages/${encodeDocId(docId)}`),
    onMutate: (docId) => removeIdsFromLists(queryClient, new Set([docId])),
    onError: (error, _docId, rollback) => {
      rollback?.();
      errorToast('Delete failed', error);
    },
    onSuccess: (outcome) => {
      invalidatePages(queryClient);
      const parts = [`${formatCount(outcome.chunks_deleted)} chunks removed`];
      if (outcome.index_cleanup_pending)
        parts.push('index cleanup pending — a background task retries');
      if (outcome.recrawl_risk) parts.push('the ACTIVE connector may recrawl this page');
      toast.success('Document deleted', { description: parts.join(' · ') });
    },
  });
}

export function useBatchDelete() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ids: string[]) =>
      api.post<BatchDeleteResponse>('/pages/batch-delete', { document_ids: ids }),
    onMutate: (ids) => removeIdsFromLists(queryClient, new Set(ids)),
    onSuccess: (res, ids, rollback) => {
      if (res.failed.length > 0) {
        // Partial failure: restore everything, then drop only confirmed ids —
        // rows disappear only when the server confirmed their deletion.
        rollback?.();
        const failedIds = new Set(res.failed.map((f) => f.id));
        removeIdsFromLists(queryClient, new Set(ids.filter((id) => !failedIds.has(id))));
        toast.error(`${formatCount(res.failed.length)} of ${formatCount(ids.length)} deletes failed`, {
          description: res.failed
            .slice(0, 3)
            .map((f) => `${f.code}: ${f.id}`)
            .join('\n'),
          duration: Infinity,
        });
      } else {
        const parts = [
          `${formatCount(res.deleted)} documents · ${formatCount(res.chunks_deleted)} chunks`,
        ];
        if (res.index_cleanup_pending > 0)
          parts.push(`${res.index_cleanup_pending} index cleanups pending`);
        toast.success('Deleted', { description: parts.join(' · ') });
      }
      invalidatePages(queryClient);
    },
    onError: (error, _ids, rollback) => {
      rollback?.();
      errorToast('Batch delete failed', error);
    },
  });
}

// ---------------------------------------------------------------------------
// Connector actions (proxied through the Onyx API by the backend)
// ---------------------------------------------------------------------------

function invalidateConnectors(queryClient: ReturnType<typeof useQueryClient>) {
  void queryClient.invalidateQueries({ queryKey: ['connectors'] });
  void queryClient.invalidateQueries({ queryKey: ['indexing'] });
  void queryClient.invalidateQueries({ queryKey: ['stats'] });
}

/**
 * Pause/resume, one or many. Optimistic status flip with rollback; the toast
 * reports per-target failures rather than a blanket success.
 */
export function usePauseResume() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ ids, action }: { ids: number[]; action: 'pause' | 'resume' }) => {
      const results = await Promise.allSettled(
        ids.map((id) => api.post<ActionResponse>(`/connectors/${id}/${action}`)),
      );
      const failed = ids.filter((_, i) => results[i]?.status === 'rejected');
      return { failed, action, total: ids.length };
    },
    onMutate: ({ ids, action }) => {
      const target = action === 'pause' ? 'PAUSED' : 'ACTIVE';
      const idSet = new Set(ids);
      const snapshots = queryClient.getQueriesData<import('./types').ConnectorSummary[]>({
        queryKey: ['connectors'],
        exact: true,
      });
      queryClient.setQueryData<import('./types').ConnectorSummary[]>(
        ['connectors'],
        (prev) =>
          prev?.map((c) => (idSet.has(c.cc_pair_id) ? { ...c, status: target } : c)),
      );
      return () => {
        for (const [key, data] of snapshots) queryClient.setQueryData(key, data);
      };
    },
    onSuccess: ({ failed, action, total }) => {
      if (failed.length > 0) {
        toast.error(`${failed.length} of ${total} ${action} calls failed`, {
          description: `cc-pairs: ${failed.join(', ')}`,
          duration: Infinity,
        });
      } else {
        toast.success(action === 'pause' ? 'Paused' : 'Resumed', {
          description: total > 1 ? `${total} connectors` : undefined,
        });
      }
      invalidateConnectors(queryClient);
    },
    onError: (error, _vars, rollback) => {
      rollback?.();
      errorToast('Action failed', error);
    },
  });
}

/**
 * Crawl now — exactly one cc-pair (there is deliberately no bulk trigger).
 * A parked pair requires the caller to have shown the explainer and the user
 * to have explicitly acknowledged; `acknowledge_parked` is never set silently.
 */
export function useRunOnce(ccPairId: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: RunOnceRequest) =>
      api.post<ActionResponse>(`/connectors/${ccPairId}/run-once`, body),
    onSuccess: (res) => {
      invalidateConnectors(queryClient);
      toast.success('Crawl queued', {
        description: res.detail ?? `cc-pair ${res.cc_pair_id} — it may wait behind in-flight attempts`,
      });
    },
    onError: (error) => errorToast('Run failed', error),
  });
}

export function useConnectorPrune(ccPairId: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.post<ActionResponse>(`/connectors/${ccPairId}/prune`),
    onSuccess: (res) => {
      invalidateConnectors(queryClient);
      toast.success('Prune kicked', { description: res.detail ?? undefined });
    },
    onError: (error) => errorToast('Prune failed', error),
  });
}

export function useConnectorPatch(ccPairId: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: ConnectorPatchRequest) =>
      api.patch<ActionResponse>(`/connectors/${ccPairId}`, body),
    onSuccess: () => {
      invalidateConnectors(queryClient);
      toast.success('Connector updated');
    },
    onError: (error) => errorToast('Update failed', error),
  });
}

/** Full cc-pair deletion; the server verifies the exact `confirm_name`. */
export function useConnectorDelete(ccPairId: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (confirmName: string) =>
      api.delete<ActionResponse>(`/connectors/${ccPairId}`, { confirm_name: confirmName }),
    onSuccess: (res) => {
      invalidateConnectors(queryClient);
      toast.success('Deletion started', {
        description: res.detail ?? 'Onyx runs the deletion as a background job',
      });
    },
    onError: (error) => errorToast('Connector delete refused', error),
  });
}

/**
 * Failed-doc reindex. The response shape is not in `api_types.rs`; typed
 * loosely and surfaced verbatim.
 */
export function useTargetedReindex(ccPairId: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      api.post<Record<string, unknown>>('/indexing/targeted-reindex', {
        cc_pair_id: ccPairId,
        only_failed: true,
      }),
    onSuccess: (res) => {
      invalidateConnectors(queryClient);
      const jobId = typeof res.job_id === 'string' || typeof res.job_id === 'number' ? res.job_id : null;
      toast.success('Reindex of failed documents started', {
        description: jobId !== null ? `job ${jobId}` : undefined,
      });
    },
    onError: (error) => errorToast('Reindex failed to start', error),
  });
}

/** Hide/unhide — the honest, reversible alternative to delete. */
export function useHidePages() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ ids, hidden }: { ids: string[]; hidden: boolean }) => {
      const results = await Promise.allSettled(
        ids.map((id) => api.patch<PatchResponse>(`/pages/${encodeDocId(id)}`, { hidden })),
      );
      const failed = results.filter((r) => r.status === 'rejected').length;
      return { total: ids.length, failed, hidden };
    },
    onSuccess: ({ total, failed, hidden }) => {
      invalidatePages(queryClient);
      if (failed > 0) {
        toast.error(`${failed} of ${total} ${hidden ? 'hide' : 'unhide'} calls failed`, {
          duration: Infinity,
        });
      } else {
        toast.success(hidden ? 'Hidden from search' : 'Visible again', {
          description: `${formatCount(total)} document${total === 1 ? '' : 's'} · data kept`,
        });
      }
    },
    onError: (error) => errorToast('Hide failed', error),
  });
}
