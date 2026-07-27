import {
  ListPagesResponse,
  PageDetailResponse,
  DeletePageResponse,
  BatchDeleteResponse,
  ConnectorSummary,
} from './types';

const API_BASE = '/api/v1';

export async function fetchPages(params: {
  page?: number;
  limit?: number;
  search?: string;
  connector_id?: number | null;
  source?: string | null;
}): Promise<ListPagesResponse> {
  const queryParams = new URLSearchParams();
  if (params.page) queryParams.set('page', params.page.toString());
  if (params.limit) queryParams.set('limit', params.limit.toString());
  if (params.search && params.search.trim()) queryParams.set('search', params.search.trim());
  if (params.connector_id != null) queryParams.set('connector_id', params.connector_id.toString());
  if (params.source) queryParams.set('source', params.source);

  const res = await fetch(`${API_BASE}/pages?${queryParams.toString()}`);
  if (!res.ok) {
    throw new Error(`Failed to fetch pages: ${res.statusText}`);
  }
  return res.json();
}

export async function fetchPageDetail(id: string): Promise<PageDetailResponse> {
  // Encode ID properly for path parameter
  const encodedId = encodeURIComponent(id);
  const res = await fetch(`${API_BASE}/pages/${encodedId}`);
  if (!res.ok) {
    throw new Error(`Failed to fetch page detail for ${id}: ${res.statusText}`);
  }
  return res.json();
}

export async function deletePage(id: string): Promise<DeletePageResponse> {
  const encodedId = encodeURIComponent(id);
  const res = await fetch(`${API_BASE}/pages/${encodedId}`, {
    method: 'DELETE',
  });
  if (!res.ok) {
    throw new Error(`Failed to delete page ${id}: ${res.statusText}`);
  }
  return res.json();
}

export async function batchDeletePages(ids: string[]): Promise<BatchDeleteResponse> {
  const res = await fetch(`${API_BASE}/pages/batch-delete`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ document_ids: ids }),
  });
  if (!res.ok) {
    throw new Error(`Failed to batch delete pages: ${res.statusText}`);
  }
  return res.json();
}

export async function fetchConnectors(): Promise<ConnectorSummary[]> {
  const res = await fetch(`${API_BASE}/connectors`);
  if (!res.ok) {
    throw new Error(`Failed to fetch connectors: ${res.statusText}`);
  }
  return res.json();
}

export async function checkBackendHealth(): Promise<boolean> {
  try {
    const res = await fetch(`${API_BASE}/health`, { method: 'GET' });
    return res.ok;
  } catch {
    return false;
  }
}
