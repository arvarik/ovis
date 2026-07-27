import { describe, expect, it } from 'vitest';
import type { PruneReason } from '@/api/types';
import { chunkLabel, graceCountdown, needsTypedCount, reasonChipText } from './pruneShared';

function reason(partial: Partial<PruneReason>): PruneReason {
  return {
    detector: 'thin',
    code: 'chunkless_stub',
    detail: '',
    confidence: 0.9,
    evidence: {},
    ...partial,
  };
}

describe('reasonChipText', () => {
  it('is compact and specific per detector', () => {
    expect(
      reasonChipText(reason({ detector: 'duplicate', code: 'near_duplicate_of', confidence: 0.94 })),
    ).toBe('dup 94%');
    expect(
      reasonChipText(
        reason({
          detector: 'language',
          code: 'lang_not_allowed',
          confidence: 0.98,
          evidence: { detected: 'deu' },
        }),
      ),
    ).toBe('lang deu 0.98');
    expect(reasonChipText(reason({ detector: 'url_rule', code: 'calendar-pages' }))).toBe(
      'rule: calendar-pages',
    );
    expect(reasonChipText(reason({}))).toBe('stub');
    expect(reasonChipText(reason({ detector: 'recrawl' }))).toBe('recrawled after prune');
  });
});

describe('graceCountdown', () => {
  const now = Date.parse('2026-07-27T12:00:00Z');

  it('renders days, hours, minutes at the right granularity', () => {
    expect(graceCountdown('2026-08-03T14:30:00Z', now)).toBe('7d 2h');
    expect(graceCountdown('2026-07-27T14:10:00Z', now)).toBe('2h 10m');
    expect(graceCountdown('2026-07-27T12:05:00Z', now)).toBe('5m');
    expect(graceCountdown('2026-07-27T12:00:30Z', now)).toBe('under a minute');
  });

  it('an elapsed grace is "due now", never a negative time', () => {
    expect(graceCountdown('2026-07-27T11:59:00Z', now)).toBe('due now');
  });
});

describe('needsTypedCount', () => {
  it('gates strictly above the server big-batch limit', () => {
    expect(needsTypedCount(500, 500)).toBe(false);
    expect(needsTypedCount(501, 500)).toBe(true);
    expect(needsTypedCount(3, 500)).toBe(false);
  });
});

describe('chunkLabel', () => {
  it('null means "not counted yet", never 0', () => {
    expect(chunkLabel(null)).toBe('—');
    expect(chunkLabel(0)).toBe('0');
    expect(chunkLabel(12)).toBe('12');
  });
});
