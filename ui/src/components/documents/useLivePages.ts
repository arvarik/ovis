import { useEffect, useRef, useState } from 'react';
import { buildListParams } from '@/api/queries';
import { streamPages, type StreamDone, type StreamError } from '@/api/sse';
import type { PageListItem } from '@/api/types';
import type { PagesSearch } from '@/routes/pages';

export interface LivePagesState {
  rows: PageListItem[];
  phase: 'streaming' | 'done' | 'error';
  done: StreamDone | null;
  error: StreamError | null;
}

const EMPTY: LivePagesState = { rows: [], phase: 'streaming', done: null, error: null };

type Keyed = LivePagesState & { key: string };

/**
 * SSE live mode. Cleanup is guaranteed on unmount/param change (D6 fix).
 * The stream is finite and server-capped; the consumer must compare
 * `done.total_matched` against `rows.length` and SAY SO when truncated.
 */
export function useLivePages(
  search: PagesSearch,
  active: boolean,
  onTransportError?: () => void,
): LivePagesState {
  // State is keyed by the param set; a param change presents EMPTY instead of
  // stale rows while the new stream spins up (no sync setState in the effect).
  const [state, setState] = useState<Keyed>({ ...EMPTY, key: '' });
  const errorCb = useRef(onTransportError);
  useEffect(() => {
    errorCb.current = onTransportError;
  });

  const paramsKey = JSON.stringify(buildListParams(search));

  useEffect(() => {
    if (!active) return;

    const buffer: PageListItem[] = [];
    let frame = 0;
    const close = streamPages(JSON.parse(paramsKey), {
      onPage: (page) => {
        buffer.push(page);
        // Batch row arrival per animation frame — 10k setState calls is jank.
        if (!frame) {
          frame = requestAnimationFrame(() => {
            frame = 0;
            setState({ key: paramsKey, rows: [...buffer], phase: 'streaming', done: null, error: null });
          });
        }
      },
      onDone: (done) => {
        if (frame) cancelAnimationFrame(frame);
        frame = 0;
        setState({ key: paramsKey, rows: [...buffer], phase: 'done', done, error: null });
      },
      onError: (error) => {
        setState({ key: paramsKey, rows: [...buffer], phase: 'error', done: null, error });
        errorCb.current?.();
      },
    });

    return () => {
      if (frame) cancelAnimationFrame(frame);
      close();
    };
  }, [paramsKey, active]);

  return state.key === paramsKey && active ? state : EMPTY;
}
