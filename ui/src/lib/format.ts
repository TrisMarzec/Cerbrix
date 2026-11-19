import type { Role, Severity } from '../api/types';

const WORDS: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  warning: 'Warning',
  info: 'Info',
};

export function severityWord(s: Severity): string {
  return WORDS[s];
}

/** Severities from the most to the least severe: 0 is the most severe. */
export const SEVERITY_RANK: Record<Severity, number> = { critical: 0, high: 1, warning: 2, info: 3 };

/** What is shown where a value is missing. */
export const DASH = '–';

const ROLE_WORDS: Record<Role, string> = { viewer: 'Viewer', operator: 'Operator', admin: 'Admin' };

export function roleWord(role: Role): string {
  return ROLE_WORDS[role];
}

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

/** Whole seconds between two instants (elapsed time), never negative; null for an unparsable start. */
function secondsSince(iso: string, to: Date): number | null {
  const from = new Date(iso).getTime();
  const end = to.getTime();
  if (Number.isNaN(from) || Number.isNaN(end)) return null;
  return Math.max(0, Math.floor((end - from) / 1000));
}

export function duration(fromIso: string, to: Date): string {
  const s = secondsSince(fromIso, to);
  if (s === null) return DASH;
  if (s < MINUTE) return `${s} s`;
  if (s < HOUR) return `${Math.floor(s / MINUTE)} min`;
  if (s < DAY) {
    const h = Math.floor(s / HOUR);
    const m = Math.floor((s % HOUR) / MINUTE);
    return m === 0 ? `${h} h` : `${h} h ${m} min`;
  }
  const d = Math.floor(s / DAY);
  const h = Math.floor((s % DAY) / HOUR);
  return h === 0 ? `${d} d` : `${d} d ${h} h`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const TIME = new Intl.DateTimeFormat('en-GB', {
  hour: '2-digit',
  minute: '2-digit',
  hourCycle: 'h23',
});

export function clock(iso: string, now: Date): string {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return DASH;
  const time = TIME.format(t);
  const sameDay =
    t.getFullYear() === now.getFullYear() &&
    t.getMonth() === now.getMonth() &&
    t.getDate() === now.getDate();
  if (sameDay) return time;
  const day = `${t.getDate()} ${MONTHS[t.getMonth()] ?? ''}`;
  return t.getFullYear() === now.getFullYear()
    ? `${day} ${time}`
    : `${day} ${t.getFullYear()} ${time}`;
}

export function age(iso: string | null, now: Date): string {
  if (iso === null) return 'never';
  const s = secondsSince(iso, now);
  if (s === null) return DASH;
  if (s < MINUTE) return `${s} s ago`;
  if (s < HOUR) return `${Math.floor(s / MINUTE)} min ago`;
  if (s < DAY) return `${Math.floor(s / HOUR)} h ago`;
  return `${Math.floor(s / DAY)} d ago`;
}

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

/** Binary units; the unit is chosen after rounding, so "1024 B" and "1024.0 KiB" never appear. */
function bytes(v: number): string {
  let n = v;
  let unit = 0;
  for (;;) {
    const shown = unit === 0 ? Math.round(n) : Number(n.toFixed(1));
    if (Math.abs(shown) >= 1024 && unit < UNITS.length - 1) {
      n /= 1024;
      unit += 1;
    } else {
      break;
    }
  }
  return unit === 0 ? `${Math.round(n)} B` : `${n.toFixed(1)} ${UNITS[unit]}`;
}

const BYTE_SUFFIXES = ['.used', '.total', '.free', '.available', '_bytes'];

export function value(key: string, v: number | null): string {
  if (v === null || !Number.isFinite(v)) return DASH;
  if (key.endsWith('_percent') || key === 'cpu.usage') return `${v.toFixed(1)} %`;
  if (BYTE_SUFFIXES.some((suffix) => key.endsWith(suffix))) return bytes(v);
  return String(Number(v.toFixed(3)));
}
