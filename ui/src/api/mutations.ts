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
  PruneBulkResponse,
  PruneRuleItem,
  PruneRulePreviewResponse,
  PruneScanItem,
  PruneScanRequest,
  PruneSelector,
  BatchDeleteResponse,
  ConnectorPatchRequest,
  DeleteOutcome,
  ListResponse,
  PageListItem,
  PagePatch,
  PatchResponse,
  RunOnceRequest,
  PrunePolicy,
  PruneCommitResponse,
  TrashBulkResponse,
  LlmProvider,
  LlmProbeResult,
  LlmRoles,
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

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

function invalidatePrune(queryClient: ReturnType<typeof useQueryClient>) {
  void queryClient.invalidateQueries({ queryKey: ['prune'] });
  // Staging flips `hidden`; deletion moves totals.
  void queryClient.invalidateQueries({ queryKey: ['pages'] });
  void queryClient.invalidateQueries({ queryKey: ['stats'] });
}

/**
 * A 409 on a bulk lifecycle mutation means the candidate set drifted between
 * review and action; the server changed nothing and its message carries the
 * fresh count. Surfaced as its own toast so the user re-confirms, never
 * retried silently.
 */
function pruneBulkErrorToast(title: string, error: unknown, queryClient: ReturnType<typeof useQueryClient>) {
  if (error instanceof ApiError && error.code === 'CONFLICT') {
    toast.error('The selection changed on the server', {
      description: `${error.message} Nothing was changed.`,
      duration: Infinity,
    });
    invalidatePrune(queryClient);
    return;
  }
  errorToast(title, error);
}

function reportBulk(response: PruneBulkResponse, did: string) {
  if (response.failed.length > 0) {
    toast.error(`${response.changed} of ${response.requested} ${did}`, {
      description: response.failed
        .slice(0, 5)
        .map((f) => `${f.document_id} — ${f.code}`)
        .join('\n'),
      duration: Infinity,
    });
  } else {
    const via =
      response.boost_hidden_via === 'onyx_api'
        ? 'hidden via Onyx API'
        : response.boost_hidden_via === 'direct_sql'
          ? 'hidden directly (no Onyx key)'
          : null;
    const grace = response.stage_expires_at
      ? `grace ends ${new Date(response.stage_expires_at).toLocaleDateString()}`
      : null;
    toast.success(`${formatCount(response.changed)} ${did}`, {
      description: [via, grace].filter(Boolean).join(' · ') || undefined,
    });
  }
}

export function usePruneStage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: PruneSelector & { confirm_count: number }) =>
      api.post<PruneBulkResponse>('/prune/candidates/stage', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      reportBulk(res, 'staged — hidden from search, data intact');
    },
    onError: (error) => pruneBulkErrorToast('Stage failed', error, queryClient),
  });
}

export function usePruneDismiss() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: PruneSelector & { exclude_future: boolean }) =>
      api.post<PruneBulkResponse>('/prune/candidates/dismiss', body),
    onSuccess: (res, body) => {
      invalidatePrune(queryClient);
      reportBulk(res, body.exclude_future ? 'dismissed, never to be re-flagged' : 'dismissed');
    },
    onError: (error) => pruneBulkErrorToast('Dismiss failed', error, queryClient),
  });
}

export function usePruneRestore() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: PruneSelector) =>
      api.post<PruneBulkResponse>('/prune/candidates/restore', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      reportBulk(res, 'restored exactly as before staging');
    },
    onError: (error) => pruneBulkErrorToast('Restore failed', error, queryClient),
  });
}

export function usePruneScheduleDelete() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: PruneSelector & { confirm_count: number; remember?: boolean }) =>
      api.post<PruneBulkResponse>('/prune/candidates/schedule-delete', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      reportBulk(res, 'scheduled — the reaper deletes after the grace period');
    },
    onError: (error) => pruneBulkErrorToast('Scheduling failed', error, queryClient),
  });
}

export function usePruneScanCreate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: PruneScanRequest) => api.post<PruneScanItem>('/prune/scans', body),
    onSuccess: (scan) => {
      void queryClient.invalidateQueries({ queryKey: ['prune'] });
      toast.success(`Scan ${scan.id} queued`, {
        description: 'A scan is a preview. Nothing is hidden or deleted.',
      });
    },
    onError: (error) => errorToast('Scan failed to queue', error),
  });
}

export function usePruneScanCancel() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.post<PruneScanItem>(`/prune/scans/${id}/cancel`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['prune'] });
      toast.success('Scan cancelled', { description: 'It stops at its next checkpoint.' });
    },
    onError: (error) => errorToast('Cancel failed', error),
  });
}

export function usePruneRuleCreate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; kind: string; body: Record<string, unknown>; enabled: boolean }) =>
      api.post<PruneRuleItem>('/prune/rules', body),
    onSuccess: (rule) => {
      void queryClient.invalidateQueries({ queryKey: ['prune', 'rules'] });
      toast.success(`Rule '${rule.name}' created`, {
        description: 'Rules start disabled — preview it against live data first.',
      });
    },
    onError: (error) => errorToast('Rule creation failed', error),
  });
}

export function usePruneRulePatch() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: number; name?: string; body?: Record<string, unknown>; enabled?: boolean }) =>
      api.patch<PruneRuleItem>(`/prune/rules/${id}`, body),
    onSuccess: (rule) => {
      void queryClient.invalidateQueries({ queryKey: ['prune', 'rules'] });
      toast.success(`Rule '${rule.name}' ${rule.enabled ? 'enabled' : 'saved'}`);
    },
    onError: (error) => errorToast('Rule update failed', error),
  });
}

export function usePruneRuleDelete() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.delete<{ deleted: boolean }>(`/prune/rules/${id}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['prune', 'rules'] });
      toast.success('Rule deleted');
    },
    onError: (error) => errorToast('Rule delete failed', error),
  });
}

export function usePruneRulePreview() {
  return useMutation({
    mutationFn: (id: number) => api.post<PruneRulePreviewResponse>(`/prune/rules/${id}/preview`),
    onError: (error) => errorToast('Preview failed', error),
  });
}

export function usePruneConfigImport() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (yaml: string) => api.putText<PruneRuleItem>('/prune/config', yaml, 'application/yaml'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['prune'] });
      toast.success('Detector config imported', {
        description: "Stored as the enabled detector_config rule 'default'.",
      });
    },
    onError: (error) => errorToast('Config import failed', error),
  });
}

// --- prune v2: policy commit and trash ---

export function usePruneCommitPolicy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      tier?: string;
      policy?: PrunePolicy;
      band: string;
      confirm_count: number;
      save_as?: string;
    }) => api.post<PruneCommitResponse>('/prune/policies/commit', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      toast.success(`${formatCount(res.created)} candidates created`, {
        description:
          res.skipped > 0
            ? `${formatCount(res.skipped)} skipped — already under review or excluded. Nothing is hidden or deleted yet.`
            : 'Nothing is hidden or deleted yet — review, then stage.',
      });
    },
    onError: (error) => pruneBulkErrorToast('Commit failed', error, queryClient),
  });
}

export function useTrashRestore() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      document_ids?: string[];
      filter?: Record<string, unknown>;
      confirm_count?: number;
      overwrite?: boolean;
    }) => api.post<TrashBulkResponse>('/prune/trash/restore', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      const pending = res.outcomes.filter((o) => o.index_restore_pending).length;
      toast.success(`${formatCount(res.changed)} restored`, {
        description: pending
          ? `${formatCount(pending)} still have chunks queued for re-indexing; they are back in Onyx and searchable once that drains.`
          : 'Back in Onyx with their chunks and vectors — searchable immediately.',
      });
      const firstFailure = res.failed[0];
      if (firstFailure) {
        toast.error(`${formatCount(res.failed.length)} could not be restored`, {
          description: firstFailure.message,
        });
      }
    },
    onError: (error) => errorToast('Restore failed', error),
  });
}

export function useTrashPurge() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      document_ids?: string[];
      filter?: Record<string, unknown>;
      confirm_count?: number;
      typed_count: number;
    }) => api.post<TrashBulkResponse>('/prune/trash/purge', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      toast.success(`${formatCount(res.changed)} permanently destroyed`, {
        description: 'This cannot be undone.',
      });
      const firstFailure = res.failed[0];
      if (firstFailure) {
        toast.error(`${formatCount(res.failed.length)} skipped`, {
          description: firstFailure.message,
        });
      }
    },
    onError: (error) => errorToast('Purge failed', error),
  });
}

export function useTrashHold() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { document_ids: string[]; hold: boolean }) =>
      api.post<TrashBulkResponse>('/prune/trash/hold', body),
    onSuccess: (res) => {
      invalidatePrune(queryClient);
      toast.success(
        res.action === 'held'
          ? `${formatCount(res.changed)} held — exempt from automatic purge`
          : `${formatCount(res.changed)} released back to the retention clock`,
      );
    },
    onError: (error) => errorToast('Hold failed', error),
  });
}

// --- llm providers, probes and roles ---

export function useLlmProviderCreate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      name: string;
      kind: string;
      base_url?: string;
      api_key_ref?: string;
    }) => api.post<LlmProvider>('/llm/providers', body),
    onSuccess: (provider) => {
      void queryClient.invalidateQueries({ queryKey: ['llm'] });
      toast.success(`${provider.name} connected`, {
        description: `${formatCount(provider.models)} model${provider.models === 1 ? '' : 's'} found. Probe them to see what each one can actually do.`,
      });
    },
    onError: (error) => errorToast('Could not connect', error),
  });
}

export function useLlmProviderDelete() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.delete<{ deleted: boolean }>(`/llm/providers/${id}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['llm'] });
      toast.success('Provider removed');
    },
    onError: (error) => errorToast('Could not remove provider', error),
  });
}

export function useLlmProbe() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ providerId, modelId }: { providerId: number; modelId: string }) =>
      api.post<LlmProbeResult>(`/llm/models/${providerId}/probe`, { model_id: modelId }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: ['llm'] });
      if (result.usable_as_judge) {
        toast.success(`${result.model_id} can be used`, { description: result.summary });
      } else {
        // Not an error — a finding. The model stays listed with its reason.
        toast.warning(`${result.model_id} cannot judge documents`, {
          description:
            result.capabilities.notes[0] ??
            'No output constraint held, so a document could make it emit arbitrary text.',
        });
      }
    },
    onError: (error) => errorToast('Probe failed', error),
  });
}

export function useLlmProbeAll() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (providerId: number) =>
      api.post<{ probed: number; usable_as_judge: number; skipped_embedding: number }>(
        `/llm/providers/${providerId}/probe`,
      ),
    onSuccess: (res) => {
      void queryClient.invalidateQueries({ queryKey: ['llm'] });
      toast.success(`Probed ${formatCount(res.probed)}`, {
        description: `${formatCount(res.usable_as_judge)} can judge documents${res.skipped_embedding ? `, ${formatCount(res.skipped_embedding)} embedding models skipped` : ''}.`,
      });
    },
    onError: (error) => errorToast('Probing failed', error),
  });
}

export function useLlmAssignRole() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { role: string; provider_id?: number; model_id?: string }) =>
      api.put<LlmRoles>('/llm/roles', body),
    onSuccess: (_roles, body) => {
      void queryClient.invalidateQueries({ queryKey: ['llm'] });
      toast.success(
        body.model_id ? `${body.role} → ${body.model_id}` : `${body.role} cleared`,
      );
    },
    onError: (error) => errorToast('Could not assign role', error),
  });
}
