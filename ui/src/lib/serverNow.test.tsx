import { act, render } from '@testing-library/preact';

import { recordServerDate, resetServerSkew } from '../api/client';
import { NOW } from '../test/fixtures';
import { advance } from '../test/timers';
import { useServerNow } from './serverNow';

function Clock({ everyMs }: { everyMs: number }) {
  const now = useServerNow(everyMs);
  return <p>{now.toISOString()}</p>;
}

function shown(container: Element): string {
  return container.querySelector('p')?.textContent ?? '';
}

function setHidden(hidden: boolean) {
  Object.defineProperty(document, 'hidden', { configurable: true, value: hidden });
  document.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  resetServerSkew();
  Reflect.deleteProperty(document, 'hidden');
  vi.useRealTimers();
});

describe('useServerNow', () => {
  it('is the browser time corrected by the recorded server clock difference', () => {
    recordServerDate(new Date(NOW.getTime() + 90_000));
    const { container } = render(<Clock everyMs={5000} />);
    expect(shown(container)).toBe(new Date(NOW.getTime() + 90_000).toISOString());
  });

  it('moves on at the given interval', async () => {
    const { container, unmount } = render(<Clock everyMs={5000} />);
    await advance(4999);
    expect(shown(container)).toBe(NOW.toISOString());
    await advance(1);
    expect(shown(container)).toBe(new Date(NOW.getTime() + 5000).toISOString());
    unmount();

    const other = render(<Clock everyMs={1000} />);
    await advance(1000);
    expect(shown(other.container)).toBe(new Date(NOW.getTime() + 6000).toISOString());
  });

  it('moves on at once when the tab becomes visible', () => {
    const { container } = render(<Clock everyMs={60_000} />);
    vi.setSystemTime(NOW.getTime() + 30_000);
    act(() => setHidden(false));
    expect(shown(container)).toBe(new Date(NOW.getTime() + 30_000).toISOString());
  });

  it('leaves no timer and no listener after unmount', async () => {
    const remove = vi.spyOn(document, 'removeEventListener');
    const { unmount } = render(<Clock everyMs={5000} />);
    // Preact runs effects after its next frame (a 35 ms timer) and cleanups likewise.
    await advance(100);
    expect(vi.getTimerCount()).toBe(1);
    unmount();
    await advance(100);
    expect(vi.getTimerCount()).toBe(0);
    expect(remove).toHaveBeenCalledWith('visibilitychange', expect.any(Function));
    remove.mockRestore();
  });
});
