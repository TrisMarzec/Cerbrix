import { isLost, LOST_MS } from './lost';

describe('isLost', () => {
  const at = { mono: 1000, wall: 1_000_000 };

  it('is lost exactly 15 s after the answer on the monotonic clock', () => {
    expect(isLost(at, { mono: 1000 + LOST_MS - 1, wall: 1_000_000 + LOST_MS - 1 })).toBe(false);
    expect(isLost(at, { mono: 1000 + LOST_MS, wall: 1_000_000 + LOST_MS })).toBe(true);
  });

  it('is lost when the wall clock moved on by 15 s while the monotonic one did not (a machine that slept)', () => {
    expect(isLost(at, { mono: 2000, wall: 1_000_000 + 3_600_000 })).toBe(true);
  });

  it('is not delayed by a wall clock that jumped back', () => {
    expect(isLost(at, { mono: 1000 + LOST_MS, wall: 1_000_000 - 3_600_000 })).toBe(true);
  });
});
