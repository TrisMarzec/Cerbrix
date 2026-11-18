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

/** A range of the API in words: "1h" is "1 hour", "7d" "7 days"; anything else stays as it is. */
export function rangeWords(range: string): string {
  const m = /^(\d+)([smhd])$/.exec(range);
  const words = m === null ? undefined : UNIT_WORDS[m[2] ?? ''];
  if (m === null || words === undefined) return range;
  const n = Number(m[1]);
  return `${n} ${n === 1 ? words[0] : words[1]}`;
}
