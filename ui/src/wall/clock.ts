import { useEffect, useMemo, useState } from 'preact/hooks';

import { serverSkewMs } from '../api/client';

/**
 * The wall's clock: the browser's time corrected by the server's clock (the `Date` header of the
 * last answer), so the clock and every "since" compare with the server's timestamps. It moves on
 * every `everyMs` and at once when the tab becomes visible (a hidden tab's timers are slowed).
 */
export function useWallNow(everyMs = 5000): Date {
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
  // A newer difference applies at the next render, without waiting for the next tick.
  const skew = serverSkewMs();
  return useMemo(() => new Date(tick + skew), [tick, skew]);
}
