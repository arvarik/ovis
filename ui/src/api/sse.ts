import { PageListItem } from './types';

export interface SSEStreamOptions {
  connector_id?: number | null;
  source?: string | null;
  search?: string;
  limit?: number;
  onPage: (page: PageListItem) => void;
  onDone: (summary: { total_matched: number; time_ms: number }) => void;
  onError: (error: Event) => void;
}

export function subscribeToPagesStream(options: SSEStreamOptions): () => void {
  const queryParams = new URLSearchParams();
  if (options.limit) queryParams.set('limit', options.limit.toString());
  if (options.search && options.search.trim()) queryParams.set('search', options.search.trim());
  if (options.connector_id != null) queryParams.set('connector_id', options.connector_id.toString());
  if (options.source) queryParams.set('source', options.source);

  const url = `/api/v1/pages/stream?${queryParams.toString()}`;
  const eventSource = new EventSource(url);

  eventSource.addEventListener('page', (event: MessageEvent) => {
    try {
      const data: PageListItem = JSON.parse(event.data);
      options.onPage(data);
    } catch (e) {
      console.error('Error parsing SSE page payload:', e);
    }
  });

  eventSource.addEventListener('done', (event: MessageEvent) => {
    try {
      const data = JSON.parse(event.data);
      options.onDone(data);
      eventSource.close();
    } catch (e) {
      console.error('Error parsing SSE done payload:', e);
    }
  });

  eventSource.onerror = (err) => {
    options.onError(err);
    eventSource.close();
  };

  return () => {
    eventSource.close();
  };
}
