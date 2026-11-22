import { fireEvent, render, screen } from '@testing-library/preact';
import type { ComponentChildren } from 'preact';
import { vi } from 'vitest';

import type { SyncState } from '../api/types';
import { NOW, statusFixture } from '../test/fixtures';
import { advance } from '../test/timers';
import { WallFrame, type WallAlarm } from './WallFrame';

const COMMIT = '4f2a9c1e8d7b6a5f4e3d2c1b0a9f8e7d6c5b4a39';

function sync(overrides: Partial<SyncState> = {}): SyncState {
  return { ...statusFixture().sync, ...overrides };
}

const outOfSync = sync({
  status: 'out_of_sync',
  active_commit: COMMIT,
  last_success_at: new Date(2026, 10, 17, 14, 5).toISOString(),
  issues: [
    { file: 'hosts/db-02.json', line: 14, column: 9, reason: 'unknown item key "disk.usedpercent"' },
    { file: 'hosts/db-03.json', line: 2, column: 1, reason: 'unknown template' },
    { file: null, line: null, column: null, reason: 'duplicate id' },
  ],
});

interface Props {
  title?: string;
  sync?: SyncState;
  alarm?: WallAlarm;
  lostSince?: Date;
  serverError?: boolean;
  back?: string;
  children?: ComponentChildren;
}

function show(props: Props = {}) {
  const { title = 'Kunde A – Region 2', children = <p>content</p>, ...rest } = props;
  return render(
    <WallFrame title={title} now={NOW} {...rest}>
      {children}
    </WallFrame>,
  );
}

function frame(container: Element): HTMLElement {
  const el = container.querySelector<HTMLElement>('.wall');
  expect(el).not.toBeNull();
  return el as HTMLElement;
}

function bands(container: Element): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>('.wall-band')];
}

describe('WallFrame header', () => {
  it('shows the title and the clock', () => {
    const { container } = show();
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Kunde A – Region 2');
    const clock = container.querySelector('time.wall-clock');
    expect(clock).toHaveTextContent('14:30');
    expect(clock).toHaveAttribute('datetime', NOW.toISOString());
  });

  it('keeps a long title on one line and offers it whole as a tooltip', () => {
    const long = 'x'.repeat(200);
    show({ title: long });
    expect(screen.getByRole('heading', { level: 1 })).toHaveAttribute('title', long);
  });

  it('says "In sync" in the Git colour', () => {
    show({ sync: sync() });
    expect(screen.getByText('In sync')).toHaveClass('wall-sync', 'is-in-sync');
  });

  it('says "Out of sync" in the warning colour', () => {
    show({ sync: outOfSync });
    expect(screen.getByText('Out of sync')).toHaveClass('wall-sync', 'is-out-of-sync');
  });

  it('says "Not synchronised yet" in dim before the first synchronisation', () => {
    show({ sync: sync({ status: 'never', active_commit: null, last_success_at: null }) });
    expect(screen.getByText('Not synchronised yet')).toHaveClass('wall-sync', 'is-never');
  });

  it('says nothing about the sync without a sync state', () => {
    const { container } = show();
    expect(container.querySelector('.wall-sync')).toBeNull();
    expect(bands(container)).toHaveLength(0);
  });

  it('is a plain header whose only live region is the sync or connection state', () => {
    const { container } = show({ sync: sync() });
    const header = container.querySelector('header.wall-header');
    expect(header).not.toBeNull();
    expect(header).not.toHaveAttribute('role');
    const live = header?.querySelectorAll('[role="status"]') ?? [];
    expect(live).toHaveLength(1);
    expect(live[0]).toHaveTextContent(/^In sync$/);
    // Neither the clock nor the title is inside a live region.
    expect(container.querySelector('time.wall-clock')?.closest('[role="status"]')).toBeNull();
    expect(screen.getByRole('heading', { level: 1 }).closest('[role="status"]')).toBeNull();
  });
});

describe('WallFrame Out Of Sync band', () => {
  it('says since when and which commit, and the first issue with the number of the others', () => {
    const { container } = show({ sync: outOfSync });
    const band = bands(container);
    expect(band).toHaveLength(1);
    expect(band[0]).toHaveClass('is-out-of-sync');
    expect(band[0]).toHaveAttribute('role', 'status');
    const lines = band[0]?.querySelectorAll('.wall-band-line') ?? [];
    expect(lines[0]).toHaveTextContent('Out of sync since 14:05, showing the configuration from commit 4f2a9c1');
    expect(lines[1]?.querySelector('.wall-band-text')).toHaveTextContent(
      'hosts/db-02.json:14:9 — unknown item key "disk.usedpercent"',
    );
    expect(lines[1]?.querySelector('.wall-band-more')).toHaveTextContent('and 2 more');
  });

  it('leaves out the time without a successful synchronisation', () => {
    const { container } = show({ sync: { ...outOfSync, last_success_at: null } });
    expect(container.querySelector('.wall-band-line')).toHaveTextContent(
      /^Out of sync, showing the configuration from commit 4f2a9c1$/,
    );
  });

  it('says that no configuration is active without an active commit', () => {
    const { container } = show({ sync: { ...outOfSync, active_commit: null, last_success_at: null } });
    expect(container.querySelector('.wall-band-line')).toHaveTextContent(/^Out of sync, no configuration is active$/);
  });

  it('leaves out the parts of the position that are unknown', () => {
    const { container } = show({
      sync: { ...outOfSync, issues: [{ file: 'hosts/a.json', line: null, column: null, reason: 'bad' }] },
    });
    const detail = container.querySelectorAll('.wall-band-line')[1];
    expect(detail?.querySelector('.wall-band-text')).toHaveTextContent(/^hosts\/a\.json — bad$/);
    expect(detail?.querySelector('.wall-band-more')).toBeNull();
  });

  it('shows only the reason without any position', () => {
    const { container } = show({
      sync: { ...outOfSync, issues: [{ file: null, line: null, column: null, reason: 'duplicate id' }] },
    });
    expect(container.querySelectorAll('.wall-band-line')[1]).toHaveTextContent(/^duplicate id$/);
  });

  it('has one line without issues', () => {
    const { container } = show({ sync: { ...outOfSync, issues: [] } });
    expect(container.querySelectorAll('.wall-band-line')).toHaveLength(1);
  });

  it('is absent while in sync and before the first synchronisation', () => {
    const { container, rerender } = show({ sync: sync() });
    expect(bands(container)).toHaveLength(0);
    rerender(
      <WallFrame title="x" now={NOW} sync={sync({ status: 'never' })}>
        <p>content</p>
      </WallFrame>,
    );
    expect(bands(container)).toHaveLength(0);
  });
});

describe('WallFrame switch band', () => {
  it.each([
    [{ showing: 'Kunde B', waiting: 0 }, 'Alarm: showing Kunde B.'],
    [{ showing: 'Kunde B', waiting: 1 }, 'Alarm: showing Kunde B. 1 more dashboard waiting.'],
    [{ showing: 'Kunde B', waiting: 2 }, 'Alarm: showing Kunde B. 2 more dashboards waiting.'],
    [{ showing: 'Kunde B', switchedFrom: 'Kunde A', waiting: 0 }, 'Alarm: showing Kunde B. Switched from Kunde A.'],
    [
      { showing: 'Kunde B', switchedFrom: 'Kunde A', waiting: 2 },
      'Alarm: showing Kunde B. Switched from Kunde A. 2 more dashboards waiting.',
    ],
  ])('says %o as "%s"', (alarm, text) => {
    const { container } = show({ alarm });
    const band = bands(container);
    expect(band).toHaveLength(1);
    expect(band[0]).toHaveClass('is-alarm');
    expect(band[0]).toHaveAttribute('role', 'status');
    expect(band[0]).toHaveTextContent(new RegExp(`^${text.replace(/[.]/g, '\\.')}$`));
  });

  it('keeps the waiting count whole in its own element, so two long names take the ellipsis', () => {
    const showing = 'Customer A – Frankfurt data centre north';
    const switchedFrom = 'Customer B – Hamburg data centre (south)';
    expect([showing.length, switchedFrom.length]).toEqual([40, 40]);
    const { container } = show({ alarm: { showing, switchedFrom, waiting: 3 } });
    const line = container.querySelector('.wall-band.is-alarm .wall-band-line');
    const text = line?.querySelector('.wall-band-text');
    const more = line?.querySelector('.wall-band-more');
    expect(text).toHaveTextContent(`Alarm: showing ${showing}. Switched from ${switchedFrom}.`);
    expect(more).toHaveTextContent(/^3 more dashboards waiting\.$/);
    // The count comes after the names, outside the element that is cut.
    expect(text?.contains(more ?? null)).toBe(false);
    expect(line?.lastElementChild).toBe(more);
  });

  it('names the most urgent problem on a second line', () => {
    const problem = { severity: 'critical', host: 'web-01', summary: 'Disk almost full', more: 0 } as const;
    const { container } = show({ alarm: { showing: 'Kunde B', waiting: 1, problem } });
    const lines = container.querySelectorAll('.wall-band.is-alarm .wall-band-line');
    expect(lines).toHaveLength(2);
    expect(lines[0]).toHaveTextContent(/^Alarm: showing Kunde B\. 1 more dashboard waiting\.$/);
    expect(lines[1]).toHaveClass('wall-band-detail');
    expect(lines[1]).toHaveTextContent(/^Critical: web-01 – Disk almost full$/);
    expect(lines[1]?.querySelector('.wall-band-text')).toHaveTextContent(/^Critical: web-01 – Disk almost full$/);
    expect(lines[1]?.querySelector('.wall-band-more')).toBeNull();
  });

  it('ends the problem line with how many more alarms there are, in its own element', () => {
    const problem = { severity: 'warning', host: 'db-01', summary: 'Replication lag', more: 2 } as const;
    const { container } = show({ alarm: { showing: 'Kunde B', waiting: 0, problem } });
    const line = container.querySelectorAll('.wall-band.is-alarm .wall-band-line')[1];
    const text = line?.querySelector('.wall-band-text');
    const more = line?.querySelector('.wall-band-more');
    expect(text).toHaveTextContent(/^Warning: db-01 – Replication lag$/);
    expect(more).toHaveTextContent(/^\+2 more$/);
    // The text of the status region reads with a space before the count.
    expect(line?.textContent).toBe('Warning: db-01 – Replication lag +2 more');
    expect(text?.contains(more ?? null)).toBe(false);
    expect(line?.lastElementChild).toBe(more);
  });

  it('has one line without a problem', () => {
    const { container } = show({ alarm: { showing: 'Kunde B', waiting: 0 } });
    expect(container.querySelectorAll('.wall-band.is-alarm .wall-band-line')).toHaveLength(1);
    expect(container.querySelector('.wall-band.is-alarm .wall-band-detail')).toBeNull();
  });

  it('comes before the Out Of Sync band', () => {
    const { container } = show({ sync: outOfSync, alarm: { showing: 'Kunde B', waiting: 0 } });
    const band = bands(container);
    expect(band.map((b) => b.classList.contains('is-alarm'))).toEqual([true, false]);
    expect(band[1]).toHaveClass('is-out-of-sync');
  });
});

describe('WallFrame without connection', () => {
  const LOST = new Date(2026, 10, 17, 14, 29, 0);

  it('says since when in place of the sync state and dims the content', () => {
    const { container } = show({ sync: sync(), lostSince: LOST });
    const lost = screen.getByText(/^No connection to the server since/);
    expect(lost).toHaveTextContent('No connection to the server since 14:29');
    expect(lost).toHaveClass('is-lost');
    expect(screen.queryByText('In sync')).toBeNull();
    expect(lost.closest('[role="status"]')).not.toBeNull();
    expect(container.querySelector('.wall-body')).toHaveClass('is-lost');
  });

  it('says that the server reported an error, with the same warning and dimming, for an answered error', () => {
    const { container } = show({ sync: sync(), lostSince: LOST, serverError: true });
    const lost = screen.getByText(/^The server reported an error since/);
    expect(lost).toHaveTextContent('The server reported an error since 14:29');
    expect(lost).toHaveClass('is-lost');
    expect(screen.queryByText(/No connection/)).toBeNull();
    expect(container.querySelector('.wall-body')).toHaveClass('is-lost');
  });

  it('dims the bands together with the content', () => {
    const { container } = show({ sync: outOfSync, alarm: { showing: 'B', waiting: 0 }, lostSince: LOST });
    const body = container.querySelector('.wall-body');
    expect(body).toHaveClass('is-lost');
    expect(body?.querySelector('.wall-band.is-alarm')).not.toBeNull();
    expect(body?.querySelector('.wall-band.is-out-of-sync')).not.toBeNull();
    expect(body?.querySelector('.wall-content')).toHaveTextContent('content');
    // The header is outside: it says what is wrong and must not dim.
    expect(container.querySelector('header')?.closest('.is-lost')).toBeNull();
  });

  it('does not dim anything while connected', () => {
    const { container } = show({ sync: outOfSync });
    expect(container.querySelector('.is-lost')).toBeNull();
    expect(container.querySelector('.wall-content')).toHaveTextContent('content');
  });
});

describe('WallFrame text from the API', () => {
  it('shows a hostile title and reason as text', () => {
    const hostile = '<img src=x onerror="alert(1)">';
    const { container } = show({
      title: hostile,
      sync: { ...outOfSync, issues: [{ file: null, line: null, column: null, reason: hostile }] },
      alarm: {
        showing: hostile,
        switchedFrom: hostile,
        waiting: 0,
        problem: { severity: 'high', host: hostile, summary: hostile, more: 0 },
      },
    });
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('.is-alarm .wall-band-detail')).toHaveTextContent(`High: ${hostile} – ${hostile}`);
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(hostile);
    expect(container.querySelector('.is-out-of-sync .wall-band-detail')).toHaveTextContent(hostile);
    expect(container.querySelector('.is-alarm .wall-band-line')).toHaveTextContent(
      `Alarm: showing ${hostile}. Switched from ${hostile}.`,
    );
  });
});

describe('WallFrame cursor and focus', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('hides the cursor after 3 seconds without mouse movement and shows it again on movement', async () => {
    const { container } = show();
    const el = frame(container);
    await advance(2_999);
    expect(el).not.toHaveClass('is-idle');
    await advance(1);
    expect(el).toHaveClass('is-idle');
    fireEvent.mouseMove(el, { screenX: 10, screenY: 10 });
    await advance(0);
    expect(el).not.toHaveClass('is-idle');
    await advance(2_000);
    fireEvent.mouseMove(el, { screenX: 11, screenY: 10 });
    await advance(2_000);
    expect(el).not.toHaveClass('is-idle');
    await advance(1_000);
    expect(el).toHaveClass('is-idle');
  });

  it('ignores a move to the position of the last one', async () => {
    const { container } = show();
    const el = frame(container);
    fireEvent.mouseMove(el, { screenX: 400, screenY: 300 });
    await advance(3_000);
    expect(el).toHaveClass('is-idle');
    fireEvent.mouseMove(el, { screenX: 400, screenY: 300 });
    fireEvent.pointerMove(el, { screenX: 400, screenY: 300 });
    await advance(0);
    expect(el).toHaveClass('is-idle');
    fireEvent.pointerMove(el, { screenX: 401, screenY: 300 });
    await advance(0);
    expect(el).not.toHaveClass('is-idle');
  });

  it.each([
    ['a key press', (el: HTMLElement) => fireEvent.keyDown(el, { key: 'Tab' })],
    ['a pointer press (touch)', (el: HTMLElement) => fireEvent.pointerDown(el)],
    ['a pointer move', (el: HTMLElement) => fireEvent.pointerMove(el, { screenX: 5, screenY: 5 })],
  ])('wakes on %s', async (_, wake) => {
    const { container } = show({ back: '/dashboards' });
    const el = frame(container);
    await advance(3_000);
    expect(el).toHaveClass('is-idle');
    wake(el);
    await advance(0);
    expect(el).not.toHaveClass('is-idle');
    await advance(3_000);
    expect(el).toHaveClass('is-idle');
  });

  it('lets the back link take focus while idle', async () => {
    const { container } = show({ back: '/dashboards' });
    await advance(3_000);
    expect(frame(container)).toHaveClass('is-idle');
    const link = screen.getByRole('link', { name: 'Back to the desk' });
    link.focus();
    expect(link).toHaveFocus();
    // That it is then visible is the rule `.wall.is-idle .wall-back:focus-visible` (wall.test.ts).
    expect(link).toHaveClass('wall-back');
  });

  it('removes the same listeners it added, and its timer, on unmount', async () => {
    const added = vi.spyOn(window, 'addEventListener');
    const removed = vi.spyOn(window, 'removeEventListener');
    const { unmount } = show();
    const wakes = added.mock.calls.filter(([type]) =>
      ['mousemove', 'pointermove', 'pointerdown', 'keydown'].includes(type),
    );
    expect(wakes.map(([type]) => type).sort()).toEqual(['keydown', 'mousemove', 'pointerdown', 'pointermove']);
    unmount();
    // Preact runs the cleanup after its next frame (a 35 ms timer); let it fire. The idle timer
    // (3 s) would still be pending if the cleanup did not clear it.
    await advance(100);
    for (const [type, listener] of wakes) expect(removed).toHaveBeenCalledWith(type, listener);
    expect(vi.getTimerCount()).toBe(0);
    added.mockRestore();
    removed.mockRestore();
  });

  it('has the back link only with `back`, and nothing else focusable', () => {
    const focusable = 'a[href], button, input, select, textarea, [tabindex]';
    const without = show({ sync: outOfSync, alarm: { showing: 'B', waiting: 1 } });
    expect(without.container.querySelectorAll(focusable)).toHaveLength(0);
    without.unmount();

    const { container } = show({ sync: outOfSync, alarm: { showing: 'B', waiting: 1 }, back: '/dashboards' });
    const found = container.querySelectorAll(focusable);
    expect(found).toHaveLength(1);
    expect(screen.getByRole('link', { name: 'Back to the desk' })).toHaveAttribute('href', '/dashboards');
  });
});
