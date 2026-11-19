import { useEffect, useMemo, useState } from 'preact/hooks';

import { serverSkewMs } from '../api/client';

/**
 * The one clock of the UI: the browser's time corrected by the server's clock (the difference the
 * API client records from the `Date` header of its answers), so durations and "x ago" compare
 * server timestamps with the server's time. It moves on every `everyMs` and at once when the tab
 * becomes visible (a hidden tab's timers are slowed down); the interval ends on unmount.
 */
export function useServerNow(everyMs: number): Date {
  const [tick, setTick] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => {
      setTick(Date.now());
    }, everyMs);
    const onVisibility = () => {
      if (!document.hidden) setTick(Date.now());
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      clearInterval(id);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [everyMs]);
  // A newly recorded difference applies at the next render, without waiting for the next tick.
  const skew = serverSkewMs();
  return useMemo(() => new Date(tick + skew), [tick, skew]);
}
