/** "db-02 · disk.used_percent · mount=/": the host, the item and its parameters in key order. */
export function seriesLabel(host: string, item: string, params: Record<string, string>): string {
  const parts = Object.keys(params)
    .sort()
    .map((k) => ` · ${k}=${params[k] ?? ''}`);
  return `${host} · ${item}${parts.join('')}`;
}

const UNIT_WORDS: Record<string, [string, string]> = {
  s: ['second', 'seconds'],
  m: ['minute', 'minutes'],
  h: ['hour', 'hours'],
  d: ['day', 'days'],
};

const UNIT_SECS: Record<string, number> = { s: 1, m: 60, h: 3600, d: 86_400 };

/** A range of the API as a number and a unit letter; undefined for any other form. */
function parse(range: string): { n: number; unit: string } | undefined {
  const m = /^(\d+)([smhd])$/.exec(range);
  return m === null ? undefined : { n: Number(m[1]), unit: m[2] ?? '' };
}

/** A range of the API in words: "1h" is "1 hour", "7d" "7 days"; anything else stays as it is. */
export function rangeWords(range: string): string {
  const parsed = parse(range);
  const words = parsed === undefined ? undefined : UNIT_WORDS[parsed.unit];
  if (parsed === undefined || words === undefined) return range;
  return `${parsed.n} ${parsed.n === 1 ? words[0] : words[1]}`;
}

/** A range of the API in seconds: "6h" is 21600; undefined for a range of another form or 0. */
export function rangeSecs(range: string): number | undefined {
  const parsed = parse(range);
  const unit = parsed === undefined ? undefined : UNIT_SECS[parsed.unit];
  if (parsed === undefined || unit === undefined || parsed.n === 0) return undefined;
  return parsed.n * unit;
}

/**
 * What a graph or value panel without data says: "No data", or, when the dashboard answer says the
 * metrics store could not be read, that the metrics are not available, so an outage of the store
 * does not read as hosts that stopped sending.
 */
export function noDataText(metricsAvailable: boolean): string {
  return metricsAvailable ? 'No data' : 'Metrics are not available right now';
}
