import { describe, expect, it } from 'vitest';
import { pagesRoute } from './index';

const validate = pagesRoute.options.validateSearch as (
  s: Record<string, unknown>,
) => Record<string, unknown>;

describe('pages validateSearch', () => {
  it('keeps valid params', () => {
    expect(
      validate({
        q: 'kant',
        mode: 'hybrid',
        connector: 42,
        sort: 'chunks_desc',
        hidden: true,
        chunk_min: 11,
      }),
    ).toMatchObject({
      q: 'kant',
      mode: 'hybrid',
      connector: 42,
      sort: 'chunks_desc',
      hidden: true,
      chunk_min: 11,
    });
  });

  it('drops invalid enum values instead of sending them to the API', () => {
    const out = validate({ sort: 'newest', mode: 'search_mode' });
    expect(out.sort).toBeUndefined();
    expect(out.mode).toBeUndefined();
  });

  it('coerces stringy numbers and booleans', () => {
    const out = validate({ connector: '42', hidden: 'true' });
    expect(out.connector).toBe(42);
    expect(out.hidden).toBe(true);
  });

  it('drops empty strings', () => {
    const out = validate({ q: '', search: '' });
    expect(out.q).toBeUndefined();
    expect(out.search).toBeUndefined();
  });
});
