import type { DisplayShow, DisplayState } from '../api/types';

/** The units of a duration as the configuration writes it. */
export type EveryUnit = 's' | 'm' | 'h' | 'd';

const UNIT_SECONDS: Record<EveryUnit, number> = { s: 1, m: 60, h: 3600, d: 86_400 };

/**
 * The parts of a duration as the configuration writes it, in order: "1m30s" → 1 m, 30 s. Undefined
 * for anything else (spaces, decimals, signs, capitals, an empty text). The one parser of `every`.
 */
export function everyParts(text: string): { amount: number; unit: EveryUnit }[] | undefined {
  if (!/^(\d+[smhd])+$/.test(text)) return undefined;
  return [...text.matchAll(/(\d+)([smhd])/g)].map((m) => ({ amount: Number(m[1]), unit: m[2] as EveryUnit }));
}

/** "30s", "5m", "1h", "1m30s" → seconds; anything else → undefined. */
export function parseEvery(text: string): number | undefined {
  return everyParts(text)?.reduce((sum, part) => sum + part.amount * UNIT_SECONDS[part.unit], 0);
}

/** A rotation never moves on faster than this: an `every` that is shorter or unreadable counts as it. */
const MIN_EVERY_MS = 5000;

/** The dashboards a display goes through when nothing is wrong; a fixed display has one. */
export function rotationList(show: DisplayShow): string[] {
  return 'dashboard' in show ? [show.dashboard] : show.rotate;
}

/** How long each dashboard of a rotation is shown, in ms; undefined for a fixed display. */
export function rotationMs(show: DisplayShow): number | undefined {
  if ('dashboard' in show) return undefined;
  return Math.max(MIN_EVERY_MS, (parseEvery(show.every) ?? 0) * 1000);
}

/** The position in the rotation list and when it was reached (monotonic ms). */
export interface Rotation {
  index: number;
  since: number;
}

export interface Shown {
  /** The dashboard to show; undefined when no dashboard of the display exists. */
  dashboard: string | undefined;
  /** Set while an alarm decides what is shown: where the display came from and how many wait. */
  alarm?: { switchedFrom?: string; waiting: number };
  rotation: Rotation;
}

/**
 * While an alarm holds the rotation, `since` lies in the future: the first call without an alarm
 * starts the interrupted dashboard's interval at that moment, so it gets its full time.
 */
const HELD = Number.POSITIVE_INFINITY;

/** The first entry from `from` on (wrapping) that is not missing; undefined when all are. */
function presentFrom(list: string[], from: number, missing: ReadonlySet<string>): number | undefined {
  for (let step = 0; step < list.length; step += 1) {
    const i = (from + step) % list.length;
    const id = list[i];
    if (id !== undefined && !missing.has(id)) return i;
  }
  return undefined;
}

/**
 * What a display shows (concept section 6), from the server's state, the page's rotation, the
 * current monotonic time and the dashboards that answered 404. Pure: the caller keeps the returned
 * rotation and passes it in again.
 *
 * - The alarm target, when the server names one the display may show (and it exists), is shown;
 *   the rotation holds where it is.
 * - Otherwise the rotation's current dashboard is shown; it moves on, wrapping, when its interval is
 *   over, skipping dashboards that do not exist. One that turns out not to exist is left at once and
 *   the next gets a full interval, as does the interrupted dashboard after an alarm.
 */
export function whatToShow(
  state: DisplayState,
  rotation: Rotation,
  nowMs: number,
  missing: ReadonlySet<string>,
): Shown {
  const list = rotationList(state.show);
  const every = rotationMs(state.show);
  const start = list.length === 0 ? 0 : ((rotation.index % list.length) + list.length) % list.length;
  const found = presentFrom(list, start, missing);
  const current = found === undefined ? undefined : list[found];

  const target = state.target;
  if (target !== null && state.allowed.includes(target) && !missing.has(target)) {
    const alarm: { switchedFrom?: string; waiting: number } = { waiting: state.waiting.length };
    if (current !== undefined && current !== target) alarm.switchedFrom = current;
    return { dashboard: target, alarm, rotation: { index: found ?? start, since: HELD } };
  }

  if (found === undefined) return { dashboard: undefined, rotation: { index: start, since: nowMs } };
  let index = found;
  // Back from an alarm, or the dashboard at this place does not exist: a full interval from now.
  let since = rotation.since > nowMs || found !== start ? nowMs : rotation.since;
  if (every !== undefined && nowMs - since >= every) {
    index = presentFrom(list, index + 1, missing) ?? index;
    since = nowMs;
  }
  return { dashboard: list[index], rotation: { index, since } };
}

/** When the rotation moves on next (monotonic ms); undefined when it does not (fixed, or held). */
export function rotationDueAt(show: DisplayShow, rotation: Rotation): number | undefined {
  const every = rotationMs(show);
  return every === undefined || !Number.isFinite(rotation.since) ? undefined : rotation.since + every;
}

/** The rotation to go on with: a changed `show` setting (a configuration change) starts it over. */
export function followShow(previous: DisplayShow, show: DisplayShow, rotation: Rotation, nowMs: number): Rotation {
  return JSON.stringify(previous) === JSON.stringify(show) ? rotation : { index: 0, since: nowMs };
}

/** A rotation entry that answered 404 is tried again after this long: it may come back. */
export const MISSING_RETRY_MS = 5 * 60_000;

/** A dashboard that answered 404: when (monotonic ms), and, for the alarm target, in which state answer. */
export interface MissingEntry {
  at: number;
  /** Set for the alarm target: the number of the state answer in force when it answered 404. */
  answer?: number;
}

/** The dashboards that answered 404, for the `allowed` list they were seen with. */
export interface Missing {
  allowed: readonly string[];
  entries: ReadonlyMap<string, MissingEntry>;
}

export const NO_MISSING: Missing = { allowed: [], entries: new Map() };

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((value, i) => value === b[i]);
}

/** The entries that still count for this state: a changed `allowed` list drops them all. */
function entriesFor(missing: Missing, state: DisplayState): ReadonlyMap<string, MissingEntry> {
  return sameList(missing.allowed, state.allowed) ? missing.entries : NO_MISSING.entries;
}

/**
 * Notes that `dashboard` answered 404 at `nowMs`. `targetAnswer` is the number of the state answer
 * in force when it was asked for as the alarm target; undefined for a rotation entry.
 */
export function noteMissing(
  missing: Missing,
  state: DisplayState,
  dashboard: string,
  nowMs: number,
  targetAnswer: number | undefined,
): Missing {
  const entry: MissingEntry = targetAnswer === undefined ? { at: nowMs } : { at: nowMs, answer: targetAnswer };
  return { allowed: state.allowed, entries: new Map(entriesFor(missing, state)).set(dashboard, entry) };
}

/**
 * When holding the rotation entries out leaves the display nothing to show, they are tried again
 * after this long instead: the poll interval.
 */
export const EMPTY_RETRY_MS = 5000;

/**
 * The dashboards held out with `retryMs` as a rotation entry's retry time. The target of the newest
 * state answer is held out only by its own 404 as the target, and only until the next state answer
 * (ruling R15): a 404 it gave as a rotation entry never holds it out.
 */
function heldWith(missing: Missing, state: DisplayState, nowMs: number, answer: number, retryMs: number): Set<string> {
  const held = new Set<string>();
  for (const [dashboard, entry] of entriesFor(missing, state)) {
    const holds =
      entry.answer !== undefined
        ? answer <= entry.answer
        : dashboard !== state.target && nowMs - entry.at < retryMs;
    if (holds) held.add(dashboard);
  }
  return held;
}

/** The retry time of the rotation entries now: short when holding them out leaves nothing to show. */
function retryFor(missing: Missing, state: DisplayState, nowMs: number, answer: number): number {
  const held = heldWith(missing, state, nowMs, answer, MISSING_RETRY_MS);
  const target = state.target;
  const targetShown = target !== null && state.allowed.includes(target) && !held.has(target);
  const list = rotationList(state.show);
  const nothing = !targetShown && list.length > 0 && list.every((id) => held.has(id));
  return nothing ? EMPTY_RETRY_MS : MISSING_RETRY_MS;
}

/**
 * The dashboards held out now (rulings R10 amended, R15): a rotation entry for `MISSING_RETRY_MS`
 * after its 404, or `EMPTY_RETRY_MS` when that would leave nothing to show; the alarm target only
 * until the next state answer (`answer` is the number of the latest), so a target that came back
 * is asked for again at once. A dashboard the newest state names as the target is never held out
 * by a rotation entry's 404. A changed `allowed` list tries every dashboard again.
 */
export function stillMissing(missing: Missing, state: DisplayState, nowMs: number, answer: number): Set<string> {
  return heldWith(missing, state, nowMs, answer, retryFor(missing, state, nowMs, answer));
}

/** When the next held-out rotation entry is due to be tried again (monotonic ms); undefined without one. */
export function missingDueAt(missing: Missing, state: DisplayState, nowMs: number, answer: number): number | undefined {
  const retryMs = retryFor(missing, state, nowMs, answer);
  const due = [...entriesFor(missing, state)]
    .filter(([dashboard, entry]) => entry.answer === undefined && dashboard !== state.target)
    .map(([, entry]) => entry.at + retryMs)
    .filter((at) => at > nowMs);
  return due.length === 0 ? undefined : Math.min(...due);
}
