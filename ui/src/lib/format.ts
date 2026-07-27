import { formatDistanceToNowStrict, format as formatDate, parseISO } from 'date-fns';

/** 1646781 -> "1,646,781" */
export function count(n: number): string {
  return n.toLocaleString('en-US');
}

/** 1646781 -> "1.6M", 10006190 -> "10.0M", 943 -> "943" */
export function compact(n: number): string {
  if (Math.abs(n) >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (Math.abs(n) >= 10_000) return (n / 1_000).toFixed(1) + 'k';
  return n.toLocaleString('en-US');
}

/** 398986524672 -> "371.5 GB" (truncated, matching how the docs quote sizes). */
export function bytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = n;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  if (i === 0) return `${Math.round(value)} B`;
  return `${(Math.floor(value * 10) / 10).toFixed(1)} ${units[i]}`;
}

/** RFC3339 -> "3 days ago" (calm, no seconds churn below a minute). */
export function relative(iso: string): string {
  const d = parseISO(iso);
  const diffMs = Date.now() - d.getTime();
  if (Math.abs(diffMs) < 60_000) return 'just now';
  return formatDistanceToNowStrict(d, { addSuffix: true });
}

/** RFC3339 -> local "2026-07-26 18:32". */
export function absolute(iso: string): string {
  return formatDate(parseISO(iso), 'yyyy-MM-dd HH:mm');
}

/** Seconds -> "45s" | "12m" | "3.2h" | "2d 4h" */
export function duration(totalSecs: number): string {
  if (totalSecs < 60) return `${Math.round(totalSecs)}s`;
  const mins = totalSecs / 60;
  if (mins < 60) return `${Math.round(mins)}m`;
  const hours = mins / 60;
  if (hours < 24) return `${hours.toFixed(hours < 10 ? 1 : 0)}h`;
  const days = Math.floor(hours / 24);
  const remH = Math.round(hours - days * 24);
  return remH > 0 ? `${days}d ${remH}h` : `${days}d`;
}

/** refresh_freq_secs -> "every 30 days" */
export function frequency(secs: number): string {
  const day = 86_400;
  if (secs % day === 0 && secs >= day) {
    const d = secs / day;
    return d === 1 ? 'every day' : `every ${d} days`;
  }
  if (secs % 3600 === 0 && secs >= 3600) {
    const h = secs / 3600;
    return h === 1 ? 'every hour' : `every ${h} hours`;
  }
  const m = Math.round(secs / 60);
  return m <= 1 ? 'every minute' : `every ${m} minutes`;
}

/** "WEB" -> "web" — sources arrive upper-case from Postgres; render calm. */
export function sourceLabel(source: string): string {
  return source.toLowerCase();
}
