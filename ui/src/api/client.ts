/**
 * Typed fetch wrapper for the OVIS API.
 *
 * - Base path `/api/v1` (dev-proxied to :8080, same-origin in production).
 * - Every non-2xx carries the error envelope `{ error: { code, message,
 *   status, req_id } }` — surfaced as a typed `ApiError`, never swallowed.
 * - Document ids are URLs and must occupy exactly one percent-encoded path
 *   segment: always route them through `encodeDocId`.
 */
import type { ApiErrorBody, HealthResponse } from './types';

const BASE = '/api/v1';
const TOKEN_KEY = 'ovis:token';

export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  readonly reqId: string | null;

  constructor(code: string, message: string, status: number, reqId: string | null) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
    this.reqId = reqId;
  }
}

export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(token: string | null): void {
  try {
    if (token === null) localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Private-mode storage failure: the token just won't persist.
  }
}

/**
 * A document id is one percent-encoded path segment. `encodeURIComponent`
 * encodes `/`, `?`, `#` and `%` — the characters that would otherwise split
 * the segment or truncate the request.
 */
export function encodeDocId(id: string): string {
  return encodeURIComponent(id);
}

export type QueryParams = Record<string, string | number | boolean | undefined | null>;

function buildQuery(query?: QueryParams): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) continue;
    params.set(key, String(value));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

function baseHeaders(withJson: boolean): Record<string, string> {
  const h: Record<string, string> = {};
  if (withJson) h['Content-Type'] = 'application/json';
  const token = getToken();
  if (token) h['Authorization'] = `Bearer ${token}`;
  return h;
}

async function toApiError(res: Response): Promise<ApiError> {
  try {
    const body = (await res.json()) as ApiErrorBody;
    if (body && typeof body === 'object' && body.error && typeof body.error.code === 'string') {
      return new ApiError(body.error.code, body.error.message, res.status, body.error.req_id);
    }
  } catch {
    // fall through — not the envelope
  }
  return new ApiError(`HTTP_${res.status}`, `request failed with HTTP ${res.status}`, res.status, null);
}

interface RequestOptions {
  query?: QueryParams;
  body?: unknown;
  signal?: AbortSignal;
}

async function request<T>(method: string, path: string, opts: RequestOptions = {}): Promise<T> {
  const res = await fetch(BASE + path + buildQuery(opts.query), {
    method,
    headers: baseHeaders(opts.body !== undefined),
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    signal: opts.signal ?? null,
  });
  if (!res.ok) throw await toApiError(res);
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export const api = {
  get<T>(path: string, query?: QueryParams, signal?: AbortSignal): Promise<T> {
    return request<T>('GET', path, { query, signal });
  },

  async getText(path: string, query?: QueryParams, signal?: AbortSignal): Promise<string> {
    const res = await fetch(BASE + path + buildQuery(query), {
      headers: baseHeaders(false),
      signal: signal ?? null,
    });
    if (!res.ok) throw await toApiError(res);
    return res.text();
  },

  post<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return request<T>('POST', path, { body: body ?? {}, signal });
  },

  patch<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
    return request<T>('PATCH', path, { body, signal });
  },

  /** PUT with a JSON body — idempotent replacement, as in role assignment. */
  put<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return request<T>('PUT', path, { body: body ?? {}, signal });
  },

  /** PUT with a raw text body (the prune config YAML import). */
  async putText<T>(path: string, text: string, contentType: string): Promise<T> {
    const res = await fetch(BASE + path, {
      method: 'PUT',
      headers: { ...baseHeaders(false), 'Content-Type': contentType },
      body: text,
    });
    if (!res.ok) throw await toApiError(res);
    return (await res.json()) as T;
  },

  delete<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return request<T>('DELETE', path, { body, signal });
  },

  /**
   * `/system/health` answers 503 when any dependency is degraded — but the
   * body is still a full HealthResponse. Parse it either way; only a
   * non-health-shaped body is an error.
   */
  async health(signal?: AbortSignal): Promise<HealthResponse> {
    const res = await fetch(`${BASE}/system/health`, {
      headers: baseHeaders(false),
      signal: signal ?? null,
    });
    let body: unknown;
    try {
      body = await res.json();
    } catch {
      throw new ApiError(`HTTP_${res.status}`, `health check failed with HTTP ${res.status}`, res.status, null);
    }
    if (body && typeof body === 'object' && 'status' in body && 'postgres' in body) {
      return body as HealthResponse;
    }
    const envelope = body as ApiErrorBody;
    if (envelope?.error?.code) {
      throw new ApiError(envelope.error.code, envelope.error.message, res.status, envelope.error.req_id);
    }
    throw new ApiError(`HTTP_${res.status}`, `health check failed with HTTP ${res.status}`, res.status, null);
  },
};
