/**
 * `GET /pages/stream` (SSE) wrapper.
 *
 * The stream is FINITE: it defaults to 1,000 rows and is capped by the
 * server's OVIS_MAX_STREAM_LIMIT (10,000). The `done` event carries
 * `total_matched`; when fewer rows arrived than matched, the consumer MUST
 * say so — presenting a truncated stream as the whole set is the exact bug
 * that bit the CLI track (cli/05_AS_BUILT.md §2.1).
 *
 * EventSource cannot send an Authorization header, so the bearer token (when
 * configured) travels as `?token=` — the SSE contract supports it.
 */
import { getToken, type QueryParams } from './client';
import type { PageListItem } from './types';

export interface StreamDone {
  total_matched: number;
  time_ms: number;
}

export interface StreamError {
  code: string;
  message: string;
}

export interface StreamHandlers {
  onPage: (page: PageListItem) => void;
  onDone: (done: StreamDone, received: number) => void;
  /** A server-sent `error` event, or a transport failure. */
  onError: (error: StreamError) => void;
}

/**
 * Opens the stream and returns a cleanup function. Cleanup is idempotent and
 * MUST be called on unmount/param change — leaked EventSources were finding
 * D6 in the old UI.
 */
export function streamPages(params: QueryParams, handlers: StreamHandlers): () => void {
  const qs = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue;
    qs.set(key, String(value));
  }
  // Request the server's own ceiling; only its cap applies (the CLI lesson).
  qs.set('limit', '10000');
  const token = getToken();
  if (token) qs.set('token', token);

  const source = new EventSource(`/api/v1/pages/stream?${qs.toString()}`);
  let received = 0;
  let finished = false;

  source.addEventListener('page', (event) => {
    try {
      const page = JSON.parse((event as MessageEvent).data) as PageListItem;
      received += 1;
      handlers.onPage(page);
    } catch {
      // one malformed row is not a reason to kill the stream
    }
  });

  source.addEventListener('done', (event) => {
    finished = true;
    source.close();
    try {
      const done = JSON.parse((event as MessageEvent).data) as StreamDone;
      handlers.onDone(done, received);
    } catch {
      handlers.onDone({ total_matched: received, time_ms: 0 }, received);
    }
  });

  source.addEventListener('error', (event) => {
    // Two kinds of "error": a server-sent error event (has data), and the
    // EventSource transport erroring (no data). A finite stream must not be
    // auto-reconnected by the browser — that would restart it from the top.
    if (finished) return;
    finished = true;
    source.close();
    const data = (event as MessageEvent).data as string | undefined;
    if (data) {
      try {
        handlers.onError(JSON.parse(data) as StreamError);
        return;
      } catch {
        // fall through
      }
    }
    handlers.onError({ code: 'STREAM_FAILED', message: 'the live stream disconnected' });
  });

  return () => {
    finished = true;
    source.close();
  };
}
