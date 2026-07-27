import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SnippetText } from './DocumentList';
import { activePreset } from './PresetChips';

describe('SnippetText', () => {
  it('renders <em> highlights as elements, never as HTML injection', () => {
    render(<SnippetText snippet="before <em>Kant</em> after <em>kant</em>." />);
    const ems = screen.getAllByText(/kant/i, { selector: 'em' });
    expect(ems).toHaveLength(2);
  });

  it('treats markup other than <em> as literal text', () => {
    render(<SnippetText snippet={'x <script>alert(1)</script> <em>hit</em>'} />);
    // The script tag is inert text content, not an element.
    expect(document.querySelector('script')).toBeNull();
    expect(screen.getByText(/alert\(1\)/)).toBeInTheDocument();
  });
});

describe('activePreset', () => {
  it('detects each canned param set', () => {
    expect(activePreset({})).toBe('all');
    expect(activePreset({ chunk_min: 0, chunk_max: 0 })).toBe('stubs');
    expect(activePreset({ chunk_min: 11 })).toBe('heavy');
    expect(activePreset({ hidden: true })).toBe('hidden');
    expect(activePreset({ updated_after: '2026-07-26T00:00:00Z' })).toBe('recent');
  });

  it('reports null for hand-rolled combinations, not a wrong chip', () => {
    expect(activePreset({ chunk_min: 3, chunk_max: 5 })).toBeNull();
  });

  it('ignores non-preset filters', () => {
    expect(activePreset({ connector: 42, source: 'web' })).toBe('all');
  });
});
