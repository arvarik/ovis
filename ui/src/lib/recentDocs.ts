/** Last-inspected documents for the command palette (localStorage, max 10). */

const KEY = 'ovis:recent-docs';
const MAX = 10;

export interface RecentDoc {
  id: string;
  title: string;
}

export function getRecentDocs(): RecentDoc[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (d): d is RecentDoc =>
        typeof d === 'object' && d !== null && typeof (d as RecentDoc).id === 'string',
    );
  } catch {
    return [];
  }
}

export function pushRecentDoc(doc: RecentDoc): void {
  try {
    const list = [doc, ...getRecentDocs().filter((d) => d.id !== doc.id)].slice(0, MAX);
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    // storage unavailable — recents just don't persist
  }
}
