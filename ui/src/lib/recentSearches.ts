/** Recent content-search queries for the mobile search sheet (max 8). */

const KEY = 'ovis:recent-searches';
const MAX = 8;

export function getRecentSearches(): string[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((s): s is string => typeof s === 'string') : [];
  } catch {
    return [];
  }
}

export function pushRecentSearch(q: string): void {
  const trimmed = q.trim();
  if (!trimmed) return;
  try {
    const list = [trimmed, ...getRecentSearches().filter((s) => s !== trimmed)].slice(0, MAX);
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    // storage unavailable
  }
}
