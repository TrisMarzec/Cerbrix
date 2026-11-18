import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/preact';
import { afterAll, afterEach, vi } from 'vitest';

afterEach(() => {
  cleanup();
});

// Preact runs effects after the next frame, with a 35 ms timer as its fallback. A timer still
// pending when a test file ends would fire after jsdom is gone ("cancelAnimationFrame is not
// defined") and fail the whole run although every test passed. Let those timers fire first.
afterAll(async () => {
  vi.useRealTimers();
  await new Promise((resolve) => setTimeout(resolve, 60));
});

// uPlot asks for the device pixel ratio when it is imported; jsdom has no matchMedia.
if (typeof window.matchMedia !== 'function') {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
