import { describe, expect, it } from 'vitest';
import { absolute, bytes, compact, count, duration, frequency, relative, sourceLabel } from './format';

describe('format', () => {
  it('counts with separators', () => {
    expect(count(1646781)).toBe('1,646,781');
    expect(count(0)).toBe('0');
    expect(count(null)).toBe('—');
    expect(count(undefined)).toBe('—');
    expect(count(NaN)).toBe('—');
  });

  it('compacts large numbers', () => {
    expect(compact(1646781)).toBe('1.6M');
    expect(compact(10006190)).toBe('10.0M');
    expect(compact(105666)).toBe('105.7k');
    expect(compact(943)).toBe('943');
    expect(compact(0)).toBe('0');
    expect(compact(null)).toBe('—');
    expect(compact(undefined)).toBe('—');
  });

  it('formats bytes like the index size', () => {
    expect(bytes(398986524672)).toBe('371.5 GB');
    expect(bytes(1024)).toBe('1.0 KB');
    expect(bytes(500)).toBe('500 B');
    expect(bytes(0)).toBe('0 B');
    expect(bytes(null)).toBe('0 B');
    expect(bytes(undefined)).toBe('0 B');
  });

  it('formats durations', () => {
    expect(duration(45)).toBe('45s');
    expect(duration(150)).toBe('3m');
    expect(duration(7200)).toBe('2.0h');
    expect(duration(0)).toBe('0s');
    expect(duration(null)).toBe('0s');
    expect(duration(undefined)).toBe('0s');
  });

  it('humanizes refresh frequencies', () => {
    expect(frequency(2592000)).toBe('every 30 days');
    expect(frequency(86400)).toBe('every day');
    expect(frequency(3600)).toBe('every hour');
    expect(frequency(null)).toBe('—');
    expect(frequency(undefined)).toBe('—');
  });

  it('renders sources calm', () => {
    expect(sourceLabel('WEB')).toBe('web');
    expect(sourceLabel(null)).toBe('unknown');
    expect(sourceLabel(undefined)).toBe('unknown');
  });

  it('formats relative and absolute dates safely', () => {
    const nowIso = new Date().toISOString();
    expect(relative(nowIso)).toBe('just now');
    expect(relative(null)).toBe('—');
    expect(relative(undefined)).toBe('—');
    expect(relative('invalid-date')).toBe('—');
    expect(relative(null, 'never')).toBe('never');

    const specificIso = '2026-07-26T18:32:00.000Z';
    expect(absolute(specificIso)).toMatch(/2026-07-26 \d\d:32/);
    expect(absolute(null)).toBe('—');
    expect(absolute(undefined)).toBe('—');
    expect(absolute('not-a-date')).toBe('—');
  });
});
