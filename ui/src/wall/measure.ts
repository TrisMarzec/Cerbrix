import type { RefObject } from 'preact';
import { useLayoutEffect, useRef, useState } from 'preact/hooks';

/** A length of a computed style in pixels; 0 when it is not a number. */
export function px(value: string): number {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) ? n : 0;
}

/** The size of an element's box in pixels, unrounded; zeros when there is no element. */
export function box(el: Element | null): { width: number; height: number } {
  if (el === null) return { width: 0, height: 0 };
  const r = el.getBoundingClientRect();
  return { width: r.width, height: r.height };
}

/**
 * Measures a panel body: `measure(body)` runs after mount and whenever the body changes size (a
 * `ResizeObserver`, or the window `resize` event where there is none), and its last result is
 * `fit`; null before the first measurement. `measure` must depend on the layout only, so define it
 * outside the component. The observer or listener is released on unmount.
 */
export function useFit<E extends HTMLElement, T>(
  measure: (body: E) => T,
): { body: RefObject<E | null>; fit: T | null } {
  return useFitOn(measure, '');
}

/**
 * As `useFit`, and measures again whenever `content` changes: for samples whose text changes
 * while the element keeps its size, such as a value shown in several sizes.
 */
export function useFitOn<E extends HTMLElement, T>(
  measure: (body: E) => T,
  content: string,
): { body: RefObject<E | null>; fit: T | null } {
  const body = useRef<E>(null);
  const [fit, setFit] = useState<T | null>(null);
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
  }, [measure, content]);
  return { body, fit };
}
