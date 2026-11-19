import type { DisplayState } from '../api/types';
import { everyParts, parseEvery, whatToShow, type Rotation } from './show';

const NOTHING = new Set<string>();

function state(overrides: Partial<DisplayState> = {}): DisplayState {
  return {
    id: 'noc-left',
    name: 'NOC left',
    show: { rotate: ['a', 'b', 'c'], every: '30s' },
    switch_on_alarm: { min_severity: 'critical' },
    allowed: ['a', 'b', 'c', 'x', 'y'],
    names: { a: 'A', b: 'B', c: 'C', x: 'X', y: 'Y' },
    target: null,
    causes: [],
    waiting: [],
    sync: { status: 'in_sync', active_commit: null, last_success_at: null, last_attempt_at: null, issues: [] },
    ...overrides,
  };
}

const START: Rotation = { index: 0, since: 1000 };

describe('parseEvery', () => {
  it.each([
    ['30s', 30],
    ['5m', 300],
    ['1h', 3600],
    ['1m30s', 90],
    ['90s', 90],
    ['1d', 86_400],
    ['0s', 0],
    ['', undefined],
    ['30', undefined],
    ['s', undefined],
    ['5 min', undefined],
    ['1.5m', undefined],
    ['-5s', undefined],
    ['30S', undefined],
    ['often', undefined],
  ])('reads %j as %j seconds', (text, seconds) => {
    expect(parseEvery(text)).toBe(seconds);
  });

  it('gives the parts as written, for words', () => {
    expect(everyParts('1m30s')).toEqual([
      { amount: 1, unit: 'm' },
      { amount: 30, unit: 's' },
    ]);
    expect(everyParts('90s')).toEqual([{ amount: 90, unit: 's' }]);
    expect(everyParts('soon')).toBeUndefined();
  });
});

describe('whatToShow', () => {
  it('shows the dashboard of a fixed display and never moves on', () => {
    const fixed = state({ show: { dashboard: 'b' } });
    expect(whatToShow(fixed, START, 1000, NOTHING)).toEqual({ dashboard: 'b', rotation: START });
    expect(whatToShow(fixed, START, 1000 + 86_400_000, NOTHING)).toEqual({ dashboard: 'b', rotation: START });
  });

  it('advances the rotation exactly at its interval and wraps at the end', () => {
    const s = state();
    expect(whatToShow(s, START, 1000, NOTHING).dashboard).toBe('a');
    expect(whatToShow(s, START, 30_999, NOTHING)).toEqual({ dashboard: 'a', rotation: START });
    const second = whatToShow(s, START, 31_000, NOTHING);
    expect(second).toEqual({ dashboard: 'b', rotation: { index: 1, since: 31_000 } });
    const third = whatToShow(s, second.rotation, 61_000, NOTHING);
    expect(third).toEqual({ dashboard: 'c', rotation: { index: 2, since: 61_000 } });
    const wrapped = whatToShow(s, third.rotation, 91_000, NOTHING);
    expect(wrapped).toEqual({ dashboard: 'a', rotation: { index: 0, since: 91_000 } });
  });

  it('stays on the one dashboard of a rotation of one', () => {
    const one = state({ show: { rotate: ['b'], every: '30s' } });
    const next = whatToShow(one, START, 31_000, NOTHING);
    expect(next).toEqual({ dashboard: 'b', rotation: { index: 0, since: 31_000 } });
  });

  it.each(['1s', '0s', '4s', 'often', ''])('counts an interval of %j as 5 seconds', (every) => {
    const s = state({ show: { rotate: ['a', 'b'], every } });
    expect(whatToShow(s, START, 5999, NOTHING).dashboard).toBe('a');
    expect(whatToShow(s, START, 6000, NOTHING).dashboard).toBe('b');
  });

  it('keeps an interval of 5 seconds and more as configured', () => {
    const s = state({ show: { rotate: ['a', 'b'], every: '1m30s' } });
    expect(whatToShow(s, START, 90_999, NOTHING).dashboard).toBe('a');
    expect(whatToShow(s, START, 91_000, NOTHING).dashboard).toBe('b');
  });

  it('shows the alarm target, holds the rotation and says where it came from', () => {
    const alarm = state({ target: 'x', waiting: ['y', 'c'] });
    const shown = whatToShow(alarm, { index: 1, since: 1000 }, 20_000, NOTHING);
    expect(shown.dashboard).toBe('x');
    expect(shown.alarm).toEqual({ switchedFrom: 'b', waiting: 2 });
    expect(shown.rotation.index).toBe(1);
    // Long past the interval: still the target, still at the same place in the rotation.
    const later = whatToShow(alarm, shown.rotation, 500_000, NOTHING);
    expect(later.dashboard).toBe('x');
    expect(later.rotation.index).toBe(1);
  });

  it('says nothing about where it came from when the target is the rotation\'s current dashboard', () => {
    const shown = whatToShow(state({ target: 'a' }), START, 2000, NOTHING);
    expect(shown.dashboard).toBe('a');
    expect(shown.alarm).toEqual({ waiting: 0 });
    expect(shown.alarm).not.toHaveProperty('switchedFrom');
  });

  it('shows the target on a fixed display too', () => {
    const shown = whatToShow(state({ show: { dashboard: 'a' }, target: 'x' }), START, 2000, NOTHING);
    expect(shown).toMatchObject({ dashboard: 'x', alarm: { switchedFrom: 'a', waiting: 0 } });
  });

  it('goes back to the interrupted dashboard when the target clears, for a full interval', () => {
    const s = state();
    // At b since 31 s; the alarm comes at 50 s, 11 s before b's time is up, and ends at 200 s.
    const atB = whatToShow(s, START, 31_000, NOTHING).rotation;
    const held = whatToShow(state({ target: 'x' }), atB, 50_000, NOTHING).rotation;
    const back = whatToShow(s, held, 200_000, NOTHING);
    expect(back.dashboard).toBe('b');
    expect(whatToShow(s, back.rotation, 229_999, NOTHING).dashboard).toBe('b');
    expect(whatToShow(s, back.rotation, 230_000, NOTHING).dashboard).toBe('c');
  });

  it('follows a target that changes from one dashboard to another, still holding the rotation', () => {
    const first = whatToShow(state({ target: 'x', waiting: ['y'] }), START, 2000, NOTHING);
    expect(first).toMatchObject({ dashboard: 'x', alarm: { switchedFrom: 'a', waiting: 1 } });
    const second = whatToShow(state({ target: 'y' }), first.rotation, 7000, NOTHING);
    expect(second).toMatchObject({ dashboard: 'y', alarm: { switchedFrom: 'a', waiting: 0 } });
    expect(second.rotation.index).toBe(0);
    const back = whatToShow(state(), second.rotation, 12_000, NOTHING);
    expect(back.dashboard).toBe('a');
    expect(back.alarm).toBeUndefined();
  });

  it('skips a dashboard that does not exist', () => {
    const missing = new Set(['b']);
    const next = whatToShow(state(), START, 31_000, missing);
    expect(next).toEqual({ dashboard: 'c', rotation: { index: 2, since: 31_000 } });
  });

  it('moves on at once, with a full interval, when the shown dashboard turns out not to exist', () => {
    const atB = { index: 1, since: 31_000 };
    const next = whatToShow(state(), atB, 40_000, new Set(['b']));
    expect(next).toEqual({ dashboard: 'c', rotation: { index: 2, since: 40_000 } });
  });

  it('shows nothing when no dashboard of the rotation exists', () => {
    const all = new Set(['a', 'b', 'c']);
    expect(whatToShow(state(), START, 31_000, all).dashboard).toBeUndefined();
    expect(whatToShow(state({ show: { dashboard: 'a' } }), START, 2000, new Set(['a'])).dashboard).toBeUndefined();
  });

  it('ignores a target that the display may not show', () => {
    const shown = whatToShow(state({ target: 'elsewhere' }), START, 2000, NOTHING);
    expect(shown.dashboard).toBe('a');
    expect(shown.alarm).toBeUndefined();
  });

  it('ignores a target that does not exist and follows the rotation', () => {
    const shown = whatToShow(state({ target: 'x' }), START, 31_000, new Set(['x']));
    expect(shown.dashboard).toBe('b');
    expect(shown.alarm).toBeUndefined();
  });

  it('finds its place when the rotation got shorter', () => {
    const shorter = state({ show: { rotate: ['a', 'b'], every: '30s' } });
    expect(whatToShow(shorter, { index: 2, since: 1000 }, 2000, NOTHING).dashboard).toBe('a');
  });

  it('gives the same answer for the same inputs and changes none of them', () => {
    const s = state({ target: 'x', waiting: ['y'] });
    const rotation = { index: 1, since: 1000 };
    const missing = new Set(['c']);
    const frozen = JSON.stringify(s);
    const first = whatToShow(s, rotation, 50_000, missing);
    expect(whatToShow(s, rotation, 50_000, missing)).toEqual(first);
    expect(JSON.stringify(s)).toBe(frozen);
    expect(rotation).toEqual({ index: 1, since: 1000 });
    expect([...missing]).toEqual(['c']);
  });
});
