import type { Severity } from '../api/types';

const WORDS: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  warning: 'Warning',
  info: 'Info',
};

export function severityWord(s: Severity): string {
  return WORDS[s];
}

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

/** Whole seconds from `fromMs` to `toMs`, never negative. */
function secondsBetween(fromMs: number, toMs: number): number {
  return Math.max(0, Math.floor((toMs - fromMs) / 1000));
}

export function duration(fromIso: string, to: Date): string {
  const s = secondsBetween(new Date(fromIso).getTime(), to.getTime());
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

const MONTH = new Intl.DateTimeFormat('en-GB', { month: 'short' });
const TIME = new Intl.DateTimeFormat('en-GB', {
  hour: '2-digit',
  minute: '2-digit',
  hourCycle: 'h23',
});

export function clock(iso: string, now: Date): string {
  const t = new Date(iso);
  const time = TIME.format(t);
  const sameDay =
    t.getFullYear() === now.getFullYear() &&
    t.getMonth() === now.getMonth() &&
    t.getDate() === now.getDate();
  if (sameDay) return time;
  const day = `${t.getDate()} ${MONTH.format(t)}`;
  return t.getFullYear() === now.getFullYear()
    ? `${day} ${time}`
    : `${day} ${t.getFullYear()} ${time}`;
}

export function age(iso: string | null, now: Date): string {
  if (iso === null) return 'never';
  const s = secondsBetween(new Date(iso).getTime(), now.getTime());
  if (s < MINUTE) return `${s} s ago`;
  if (s < HOUR) return `${Math.floor(s / MINUTE)} min ago`;
  if (s < DAY) return `${Math.floor(s / HOUR)} h ago`;
  return `${Math.floor(s / DAY)} d ago`;
}

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

function bytes(v: number): string {
  let n = v;
  let unit = 0;
  while (Math.abs(n) >= 1024 && unit < UNITS.length - 1) {
    n /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${Math.round(n)} B` : `${n.toFixed(1)} ${UNITS[unit]}`;
}

const BYTE_SUFFIXES = ['.used', '.total', '.free', '.available', '_bytes'];

export function value(key: string, v: number | null): string {
  if (v === null) return '–';
  if (key.endsWith('_percent') || key === 'cpu.usage') return `${v.toFixed(1)} %`;
  if (BYTE_SUFFIXES.some((suffix) => key.endsWith(suffix))) return bytes(v);
  return String(Number(v.toFixed(3)));
}
