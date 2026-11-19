import { useEffect, useRef, useState } from 'preact/hooks';

import { serverSkewMs } from '../api/client';

/** After this long without a successful answer the wall no longer shows a poll's data as current. */
export const LOST_MS = 15_000;

/**
 * Since when a poll fails (its `staleSince`), once it has failed for `LOST_MS`; undefined before
 * that and after a success. The time that passed is measured on the monotonic clock, so a jump of
 * the browser's clock cannot delay it; a timer renders again when the limit is reached. The time
 * given back is the server's, as the wall's clock.
 */
export function useFailingFor(staleSince: Date | undefined): Date | undefined {
  const since = staleSince?.getTime();
  // When this component first saw the current failure, on the monotonic clock.
  const seen = useRef<{ since: number | undefined; at: number }>({ since: undefined, at: 0 });
  if (seen.current.since !== since) seen.current = { since, at: performance.now() };
  const [, wake] = useState(0);
  useEffect(() => {
    if (since === undefined) return undefined;
    const wait = seen.current.at + LOST_MS - performance.now();
    if (wait <= 0) return undefined;
    const timer = setTimeout(() => {
      wake((n) => n + 1);
    }, wait);
    return () => {
      clearTimeout(timer);
    };
  }, [since]);
  if (since === undefined || performance.now() - seen.current.at < LOST_MS) return undefined;
  // `staleSince` is the browser's time; the wall shows the server's.
  return new Date(since + serverSkewMs());
}
