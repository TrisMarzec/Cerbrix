import type { RefObject } from 'preact';
import { useLayoutEffect, useRef, useState } from 'preact/hooks';

/** A length of a computed style in pixels; 0 when it is not a number. */
export function px(value: string): number {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) ? n : 0;
}

/**
 * Measures a panel body: `measure(body)` runs after mount and whenever the body changes size (a
 * `ResizeObserver`, or the window `resize` event where there is none), and its last result is
 * `fit`; null before the first measurement. `measure` must depend on the layout only, so define it
 * outside the component. The observer or listener is released on unmount.
 */
export function useFit<T extends HTMLElement>(
  measure: (body: T) => number | null,
): { body: RefObject<T | null>; fit: number | null } {
  const body = useRef<T>(null);
  const [fit, setFit] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = body.current;
    if (el === null) return undefined;
    const update = () => setFit(measure(el));
    update();
    if (typeof ResizeObserver === 'function') {
      const observer = new ResizeObserver(update);
      observer.observe(el);
      return () => observer.disconnect();
    }
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [measure]);
  return { body, fit };
}
