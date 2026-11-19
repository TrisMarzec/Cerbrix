import { useEffect, useRef, useState } from 'preact/hooks';

import { serverSkewMs } from '../api/client';
import { moment, type Moment } from '../lib/poll';

/** After this long without a successful answer the wall no longer shows a poll's data as current. */
export const LOST_MS = 15_000;

/**
 * Whether an answer from `at` is too old to pass as current (ruling R17): `LOST_MS` have passed on
 * the monotonic clock, or the browser's clock moved on by that much (a machine that slept, where
 * the monotonic clock may not have advanced).
 */
export function isLost(at: Moment, now: Moment = moment()): boolean {
  return now.mono - at.mono >= LOST_MS || now.wall - at.wall >= LOST_MS;
}

/**
 * The one rule for a lost connection (ruling R17): the time of the last successful answer of a
 * resource, `answeredAt`, once it is too old (`isLost`); undefined before that. Without an answer
 * yet it counts from the mount. The time given back is the server's, as the wall's clock. Every
 * render applies the rule; a timer renders again when the monotonic limit is reached, and a tab
 * that becomes visible applies it at once (a hidden tab's timers are held back).
 */
export function useLostSince(answeredAt: Moment | undefined): Date | undefined {
  const mounted = useRef<Moment | undefined>(undefined);
  mounted.current ??= moment();
  const at = answeredAt ?? mounted.current;
  const [, wake] = useState(0);
  const now = moment();
  const lost = isLost(at, now);

  useEffect(() => {
    const onVisibility = () => {
      if (!document.hidden) wake((n) => n + 1);
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

  const dueIn = Math.min(at.mono + LOST_MS - now.mono, at.wall + LOST_MS - now.wall);
  useEffect(() => {
    if (lost) return undefined;
    const timer = setTimeout(() => wake((n) => n + 1), Math.max(0, dueIn));
    return () => clearTimeout(timer);
  }, [at.mono, at.wall, lost]);

  // The answer's time is the browser's; the wall shows the server's.
  return lost ? new Date(at.wall + serverSkewMs()) : undefined;
}
