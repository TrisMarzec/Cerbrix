import type { DisplayState } from '../api/types';
import {
  followShow,
  EMPTY_RETRY_MS,
  MISSING_RETRY_MS,
  missingDueAt,
  NO_MISSING,
  noteMissing,
  rotationDueAt,
  stillMissing,
  whatToShow,
  type Rotation,
} from './show';

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

  it('advances exactly one step when the clock jumps by hours (a sleeping tab, a suspended machine)', () => {
    const later = whatToShow(state(), START, 1000 + 5 * 3_600_000, NOTHING);
    expect(later).toEqual({ dashboard: 'b', rotation: { index: 1, since: 1000 + 5 * 3_600_000 } });
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

describe('rotationDueAt', () => {
  it('is the end of the current interval, and nothing for a fixed or held rotation', () => {
    expect(rotationDueAt({ rotate: ['a', 'b'], every: '30s' }, { index: 0, since: 1000 })).toBe(31_000);
    expect(rotationDueAt({ rotate: ['a', 'b'], every: '1s' }, { index: 0, since: 1000 })).toBe(6000);
    expect(rotationDueAt({ dashboard: 'a' }, { index: 0, since: 1000 })).toBeUndefined();
    const held = whatToShow(state({ target: 'x' }), START, 2000, NOTHING).rotation;
    expect(rotationDueAt({ rotate: ['a', 'b'], every: '30s' }, held)).toBeUndefined();
  });
});

describe('followShow', () => {
  const rotation = { index: 2, since: 1000 };

  it('keeps the rotation while the setting stays the same', () => {
    const show = { rotate: ['a', 'b', 'c'], every: '30s' };
    expect(followShow(show, { rotate: ['a', 'b', 'c'], every: '30s' }, rotation, 9000)).toBe(rotation);
  });

  it.each([
    ['another interval', { rotate: ['a', 'b', 'c'], every: '1m' }],
    ['another list', { rotate: ['a', 'c'], every: '30s' }],
    ['a fixed dashboard', { dashboard: 'a' }],
  ])('starts over at the first dashboard for %s', (_, show) => {
    const next = followShow({ rotate: ['a', 'b', 'c'], every: '30s' }, show, rotation, 9000);
    expect(next).toEqual({ index: 0, since: 9000 });
  });
});

describe('the missing dashboards', () => {
  const s = state();

  it('holds a rotation entry out for five minutes after its 404', () => {
    const missing = noteMissing(NO_MISSING, s, 'b', 1000, undefined);
    expect([...stillMissing(missing, s, 1000, 1)]).toEqual(['b']);
    expect([...stillMissing(missing, s, 1000 + MISSING_RETRY_MS - 1, 50)]).toEqual(['b']);
    expect([...stillMissing(missing, s, 1000 + MISSING_RETRY_MS, 50)]).toEqual([]);
    expect(missingDueAt(missing, s, 2000, 1)).toBe(1000 + MISSING_RETRY_MS);
    expect(missingDueAt(missing, s, 1000 + MISSING_RETRY_MS, 50)).toBeUndefined();
  });

  it('holds the alarm target out only until the next state answer', () => {
    // Asked for as the target in state answer 7.
    const missing = noteMissing(NO_MISSING, s, 'x', 1000, 7);
    expect([...stillMissing(missing, s, 1000, 7)]).toEqual(['x']);
    expect([...stillMissing(missing, s, 2000, 8)]).toEqual([]);
    // No timer for it: the next state answer decides.
    expect(missingDueAt(missing, s, 2000, 8)).toBeUndefined();
  });

  it('tries every dashboard again when the allowed list changes', () => {
    const missing = noteMissing(noteMissing(NO_MISSING, s, 'b', 1000, undefined), s, 'x', 1000, 3);
    expect(stillMissing(missing, s, 2000, 3).size).toBe(2);
    const changed = state({ allowed: ['a', 'b', 'c'] });
    expect(stillMissing(missing, changed, 2000, 3).size).toBe(0);
    expect(missingDueAt(missing, changed, 2000, 3)).toBeUndefined();
    // A 404 under the new list starts a new set.
    expect([...stillMissing(noteMissing(missing, changed, 'c', 2000, undefined), changed, 2000, 3)]).toEqual(['c']);
  });

  it('never holds out the target of the newest state for a 404 it gave as a rotation entry (R15)', () => {
    const missing = noteMissing(NO_MISSING, s, 'b', 1000, undefined);
    const alarm = state({ target: 'b' });
    expect([...stillMissing(missing, alarm, 2000, 4)]).toEqual([]);
    expect(whatToShow(alarm, START, 2000, stillMissing(missing, alarm, 2000, 4)).dashboard).toBe('b');
    // Its 404 as the target holds it until the next state answer only.
    const again = noteMissing(missing, alarm, 'b', 3000, 4);
    expect([...stillMissing(again, alarm, 3000, 4)]).toEqual(['b']);
    expect([...stillMissing(again, alarm, 3000, 5)]).toEqual([]);
  });

  it('tries the held-out entries again after 5 s when holding them out leaves nothing to show', () => {
    const fixed = state({ show: { dashboard: 'a' } });
    const missing = noteMissing(NO_MISSING, fixed, 'a', 1000, undefined);
    expect([...stillMissing(missing, fixed, 1000 + EMPTY_RETRY_MS - 1, 1)]).toEqual(['a']);
    expect([...stillMissing(missing, fixed, 1000 + EMPTY_RETRY_MS, 1)]).toEqual([]);
    expect(missingDueAt(missing, fixed, 2000, 1)).toBe(1000 + EMPTY_RETRY_MS);
    // A rotation with every entry missing, too.
    const all = ['a', 'b', 'c'].reduce((m, id) => noteMissing(m, s, id, 1000, undefined), NO_MISSING);
    expect(missingDueAt(all, s, 2000, 1)).toBe(1000 + EMPTY_RETRY_MS);
    // While something else can be shown, the five minutes stay.
    const one = noteMissing(NO_MISSING, s, 'b', 1000, undefined);
    expect(missingDueAt(one, s, 2000, 1)).toBe(1000 + MISSING_RETRY_MS);
  });

  it('changes nothing it is given', () => {
    const missing = noteMissing(NO_MISSING, s, 'b', 1000, undefined);
    noteMissing(missing, s, 'c', 2000, undefined);
    expect([...missing.entries.keys()]).toEqual(['b']);
    expect(NO_MISSING.entries.size).toBe(0);
  });
});
