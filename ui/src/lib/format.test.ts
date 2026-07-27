import { describe, expect, it } from 'vitest';
import { bytes, compact, count, duration, frequency, sourceLabel } from './format';

describe('format', () => {
  it('counts with separators', () => {
    expect(count(1646781)).toBe('1,646,781');
  });

  it('compacts large numbers', () => {
    expect(compact(1646781)).toBe('1.6M');
    expect(compact(10006190)).toBe('10.0M');
    expect(compact(105666)).toBe('105.7k');
    expect(compact(943)).toBe('943');
  });

  it('formats bytes like the index size', () => {
    expect(bytes(398986524672)).toBe('371.5 GB');
    expect(bytes(1024)).toBe('1.0 KB');
    expect(bytes(500)).toBe('500 B');
  });

  it('formats durations', () => {
    expect(duration(45)).toBe('45s');
    expect(duration(150)).toBe('3m');
    expect(duration(7200)).toBe('2.0h');
  });

  it('humanizes refresh frequencies', () => {
    expect(frequency(2592000)).toBe('every 30 days');
    expect(frequency(86400)).toBe('every day');
    expect(frequency(3600)).toBe('every hour');
  });

  it('renders sources calm', () => {
    expect(sourceLabel('WEB')).toBe('web');
  });
});
