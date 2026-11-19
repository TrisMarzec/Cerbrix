import { durationParts, durationSecs, type DurationUnit } from '../../lib/duration';

/** "db-02 · disk.used_percent · mount=/": the host, the item and its parameters in key order. */
export function seriesLabel(host: string, item: string, params: Record<string, string>): string {
  const parts = Object.keys(params)
    .sort()
    .map((k) => ` · ${k}=${params[k] ?? ''}`);
  return `${host} · ${item}${parts.join('')}`;
}

const UNIT_WORDS: Record<DurationUnit, [string, string]> = {
  s: ['second', 'seconds'],
  m: ['minute', 'minutes'],
  h: ['hour', 'hours'],
  d: ['day', 'days'],
};

/** A range of the API as one number and a unit; undefined for any other form ("1h30m" included). */
function parse(range: string): { amount: number; unit: DurationUnit } | undefined {
  const parts = durationParts(range);
  return parts?.length === 1 ? parts[0] : undefined;
}

/** A range of the API in words: "1h" is "1 hour", "7d" "7 days"; anything else stays as it is. */
export function rangeWords(range: string): string {
  const parsed = parse(range);
  if (parsed === undefined) return range;
  const words = UNIT_WORDS[parsed.unit];
  return `${parsed.amount} ${parsed.amount === 1 ? words[0] : words[1]}`;
}

/** A range of the API in seconds: "6h" is 21600; undefined for a range of another form or 0. */
export function rangeSecs(range: string): number | undefined {
  const secs = parse(range) === undefined ? undefined : durationSecs(range);
  return secs === 0 ? undefined : secs;
}

/**
 * What a graph or value panel without data says: "No data", or, when the dashboard answer says the
 * metrics store could not be read, that the metrics are not available, so an outage of the store
 * does not read as hosts that stopped sending.
 */
export function noDataText(metricsAvailable: boolean): string {
  return metricsAvailable ? 'No data' : 'Metrics are not available right now';
}
