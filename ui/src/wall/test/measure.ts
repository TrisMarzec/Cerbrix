import { act } from '@testing-library/preact';
import { vi } from 'vitest';

/** The sizes in pixels a wall panel reads from the layout, which jsdom does not compute. */
export interface Sizes {
  /** The panel body (`.wall-panel-body`): `clientWidth` and `clientHeight`. */
  bodyWidth: number;
  bodyHeight: number;
  /** The hidden sample problem row: the height of its box. */
  rowHeight: number;
  /** The hidden sample sentence of the host-states panel: the height of its box. */
  lineHeight: number;
  /** The hidden sample square: its computed width and right margin (the gap between squares). */
  square: number;
  gap: number;
}

const NONE: Sizes = { bodyWidth: 0, bodyHeight: 0, rowHeight: 0, lineHeight: 0, square: 0, gap: 0 };

class FakeResizeObserver {
  static all: FakeResizeObserver[] = [];
  callback: ResizeObserverCallback;
  targets = new Set<Element>();
  disconnected = false;
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    FakeResizeObserver.all.push(this);
  }
  observe(target: Element) {
    this.targets.add(target);
  }
  unobserve(target: Element) {
    this.targets.delete(target);
  }
  disconnect() {
    this.disconnected = true;
    this.targets.clear();
  }
}

export interface Measure {
  /** Changes the sizes; call `resize()` to tell the panel. */
  set(sizes: Partial<Sizes>): void;
  /** Fires every connected observer (or the window `resize` event without observers). */
  resize(): void;
  /** The observers created so far. */
  observers(): { targets: Set<Element>; disconnected: boolean }[];
}

let sizes: Sizes = { ...NONE };
let restore: (() => void)[] = [];

function is(el: Element, ...classes: string[]): boolean {
  return classes.every((c) => el.classList.contains(c));
}

function override<T extends object>(target: T, name: string, descriptor: PropertyDescriptor) {
  const original = Object.getOwnPropertyDescriptor(target, name);
  Object.defineProperty(target, name, { configurable: true, ...descriptor });
  restore.push(() => {
    if (original === undefined) Reflect.deleteProperty(target, name);
    else Object.defineProperty(target, name, original);
  });
}

/**
 * Makes the layout of the wall panels measurable in jsdom: the body's client size, the sample row
 * and sentence boxes and the sample square's computed width and margin come from `initial`.
 * With `resizeObserver: false` the global `ResizeObserver` is absent, as in an old browser.
 * Call `releaseMeasure()` after each test.
 */
export function installMeasure(initial: Partial<Sizes>, { resizeObserver = true } = {}): Measure {
  sizes = { ...NONE, ...initial };
  FakeResizeObserver.all = [];
  vi.stubGlobal('ResizeObserver', resizeObserver ? FakeResizeObserver : undefined);

  override(HTMLElement.prototype, 'clientWidth', {
    get(this: HTMLElement) {
      return is(this, 'wall-panel-body') ? sizes.bodyWidth : 0;
    },
  });
  override(HTMLElement.prototype, 'clientHeight', {
    get(this: HTMLElement) {
      return is(this, 'wall-panel-body') ? sizes.bodyHeight : 0;
    },
  });

  const rect = Element.prototype.getBoundingClientRect;
  override(Element.prototype, 'getBoundingClientRect', {
    value(this: Element): DOMRect {
      let height = 0;
      if (is(this, 'wall-problem', 'wall-sample')) height = sizes.rowHeight;
      else if (is(this, 'wall-hosts-sentence', 'wall-sample')) height = sizes.lineHeight;
      else return rect.call(this);
      return new DOMRect(0, 0, 0, height);
    },
  });

  const computed = window.getComputedStyle.bind(window);
  override(window, 'getComputedStyle', {
    value(el: Element, pseudo?: string | null): CSSStyleDeclaration {
      const style = computed(el, pseudo);
      if (!is(el, 'wall-square', 'wall-sample')) return style;
      return new Proxy(style, {
        get(target, prop) {
          if (prop === 'width') return `${sizes.square}px`;
          if (prop === 'marginRight') return `${sizes.gap}px`;
          const v: unknown = Reflect.get(target, prop, target);
          return typeof v === 'function' ? v.bind(target) : v;
        },
      });
    },
  });

  return {
    set(next) {
      sizes = { ...sizes, ...next };
    },
    resize() {
      act(() => {
        const live = FakeResizeObserver.all.filter((o) => !o.disconnected && o.targets.size > 0);
        if (live.length === 0) window.dispatchEvent(new Event('resize'));
        for (const o of live) o.callback([], o as unknown as ResizeObserver);
      });
    },
    observers() {
      return FakeResizeObserver.all;
    },
  };
}

/** Undoes `installMeasure`. */
export function releaseMeasure() {
  for (const undo of restore.reverse()) undo();
  restore = [];
  vi.unstubAllGlobals();
  sizes = { ...NONE };
}
