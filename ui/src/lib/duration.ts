/** The units of a duration as the configuration and the API write it. */
export type DurationUnit = 's' | 'm' | 'h' | 'd';

const UNIT_SECONDS: Record<DurationUnit, number> = { s: 1, m: 60, h: 3600, d: 86_400 };

/**
 * The parts of a duration as the configuration and the API write it, in order: "1m30s" → 1 m,
 * 30 s. Undefined for anything else (spaces, decimals, signs, capitals, an empty text). The one
 * parser of durations: a display's `every`, a graph panel's `range`.
 */
export function durationParts(text: string): { amount: number; unit: DurationUnit }[] | undefined {
  if (!/^(\d+[smhd])+$/.test(text)) return undefined;
  return [...text.matchAll(/(\d+)([smhd])/g)].map((m) => ({ amount: Number(m[1]), unit: m[2] as DurationUnit }));
}

/** "30s", "5m", "1h", "1m30s" → seconds; anything else → undefined. */
export function durationSecs(text: string): number | undefined {
  return durationParts(text)?.reduce((sum, part) => sum + part.amount * UNIT_SECONDS[part.unit], 0);
}
