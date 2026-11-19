import { fireEvent, render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';
import { vi } from 'vitest';

import { resetServerSkew } from '../api/client';
import { App } from '../app';
import { navigate } from '../lib/router';
import { NOW, dashboardDetailFixture, statusFixture } from '../test/fixtures';
import { mockFetch, type FetchMock } from '../test/mockFetch';
import { advance } from '../test/timers';
import { installMeasure, releaseMeasure } from './test/measure';
import { WallPage } from './WallPage';

vi.mock('uplot', () => ({
  default: class {
    constructor(_opts: uPlot.Options) {}
    setSize() {}
    setData() {}
    destroy() {}
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

const innerWidth = Object.getOwnPropertyDescriptor(window, 'innerWidth');
const DASHBOARD = 'GET /dashboards/kunde-a';

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1920 });
  installMeasure({
    bodyWidth: 1000,
    bodyHeight: 600,
    rowHeight: 100,
    rowWidth: 400,
    summaryHeight: 60,
    emptyWidth: 500,
    emptyHeight: 60,
    lineHeight: 40,
    square: 30,
    gap: 6,
    charWidth: 10,
  });
});

afterEach(() => {
  mock.restore();
  releaseMeasure();
  resetServerSkew();
  if (innerWidth !== undefined) Object.defineProperty(window, 'innerWidth', innerWidth);
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
  document.title = '';
});

const kundeA = dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' });

async function open(routes: Parameters<typeof mockFetch>[0], id = 'kunde-a') {
  mock = mockFetch({ 'GET /status': { json: statusFixture() }, ...routes });
  const view = render(<WallPage id={id} />);
  await advance(0);
  return view;
}

function title(): string {
  return document.querySelector('.wall-title')?.textContent ?? '';
}

function dimmed(container: Element): boolean {
  return container.querySelector('.wall-body')?.classList.contains('is-lost') === true;
}

describe('WallPage', () => {
  it('shows the dashboard on the grid with panels of all four types, without navigation', async () => {
    const { container } = await open({ [DASHBOARD]: { json: kundeA } });
    expect(screen.getByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    const labels = [...container.querySelectorAll('.wall-panel > .wall-panel-label')].map((l) => l.textContent);
    expect(labels).toEqual(['Problems', 'Hosts', expect.stringContaining('db-01 · cpu.usage'), 'db-01 · load.1m']);
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(document.title).toBe('Kunde A · Cerbrix');
  });

  it('shows the empty frame with the id as title before the first answer', async () => {
    const { container } = await open({ [DASHBOARD]: () => new Promise(() => {}) });
    expect(title()).toBe('kunde-a');
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(container.querySelector('.wall-panel')).toBeNull();
    expect(document.title).toBe('kunde-a · Cerbrix');
  });

  it('asks for the dashboard and the status every 5 seconds', async () => {
    await open({ [DASHBOARD]: { json: kundeA } });
    expect(mock.count(DASHBOARD)).toBe(1);
    expect(mock.count('GET /status')).toBe(1);
    await advance(4999);
    expect(mock.count(DASHBOARD)).toBe(1);
    await advance(1);
    expect(mock.count(DASHBOARD)).toBe(2);
    expect(mock.count('GET /status')).toBe(2);
    await advance(5000);
    expect(mock.count(DASHBOARD)).toBe(3);
  });

  it('shows a newer answer of the dashboard', async () => {
    await open({ [DASHBOARD]: [{ json: kundeA }, { json: { ...kundeA, name: 'Kunde A – Region 2' } }] });
    await advance(5000);
    expect(title()).toBe('Kunde A – Region 2');
  });

  it('keeps the picture on failures, says after 15 s without success that the connection is lost, and clears it on success', async () => {
    const { container } = await open({ [DASHBOARD]: { json: kundeA } });
    mock.on(DASHBOARD, 'network');
    await advance(5000);
    // The first failure at 14:30:05; the picture stays, without a notice.
    expect(title()).toBe('Kunde A');
    expect(container.querySelectorAll('.wall-panel')).toHaveLength(4);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    expect(dimmed(container)).toBe(false);
    await advance(14_999);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await advance(1);
    const lost = screen.getByText(/No connection to the server since/);
    expect(lost).toHaveTextContent('No connection to the server since 14:30');
    expect(lost.closest('[role="status"]')).not.toBeNull();
    expect(dimmed(container)).toBe(true);
    expect(container.querySelectorAll('.wall-panel')).toHaveLength(4);

    mock.on(DASHBOARD, { json: kundeA });
    await advance(5000);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    expect(dimmed(container)).toBe(false);
  });

  it('shows the frame with the id and, after 15 s, the lost connection when the first load fails', async () => {
    const { container } = await open({ [DASHBOARD]: 'network' });
    expect(title()).toBe('kunde-a');
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await advance(15_000);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
  });

  it('says that the server reported an error, not that the connection is lost, for answered errors', async () => {
    const { container } = await open({ [DASHBOARD]: { json: kundeA } });
    mock.on(DASHBOARD, { status: 500, json: { error: 'internal', message: 'boom' } });
    await advance(5000);
    await advance(14_999);
    expect(screen.queryByText(/reported an error|No connection/)).not.toBeInTheDocument();
    expect(dimmed(container)).toBe(false);
    await advance(1);
    expect(screen.getByText(/^The server reported an error since/)).toHaveTextContent(
      'The server reported an error since 14:30',
    );
    expect(screen.queryByText(/No connection/)).not.toBeInTheDocument();
    expect(dimmed(container)).toBe(true);
    // The picture stays, dimmed.
    expect(title()).toBe('Kunde A');
    expect(container.querySelectorAll('.wall-panel')).toHaveLength(4);
    mock.on(DASHBOARD, { json: kundeA });
    await advance(5000);
    expect(screen.queryByText(/reported an error/)).not.toBeInTheDocument();
    expect(dimmed(container)).toBe(false);
  });

  it('measures the 15 s on a monotonic clock, so a browser clock jump cannot delay the notice', async () => {
    await open({ [DASHBOARD]: { json: kundeA } });
    mock.on(DASHBOARD, 'network');
    await advance(5000);
    // The browser clock jumps an hour back right after the first failure.
    vi.setSystemTime(Date.now() - 3_600_000);
    await advance(15_000);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
  });

  it('stops showing the sync state when the status cannot be asked for 15 s, and shows it again later', async () => {
    const outOfSync = { ...statusFixture().sync, status: 'out_of_sync' as const };
    await open({ [DASHBOARD]: { json: kundeA }, 'GET /status': { json: statusFixture({ sync: outOfSync }) } });
    expect(screen.getByText('Out of sync')).toBeInTheDocument();
    expect(screen.getByText(/^Out of sync since/)).toBeInTheDocument();
    mock.on('GET /status', 'network');
    await advance(5000);
    await advance(14_999);
    // Failing for less than 15 s: the last state stays.
    expect(screen.getByText(/^Out of sync since/)).toBeInTheDocument();
    await advance(1);
    expect(screen.queryByText(/Out of sync/)).not.toBeInTheDocument();
    expect(screen.queryByText('In sync')).not.toBeInTheDocument();
    // The dashboard still answers: no lost connection, nothing dimmed.
    expect(screen.queryByText(/No connection|reported an error/)).not.toBeInTheDocument();
    expect(title()).toBe('Kunde A');
    mock.on('GET /status', { json: statusFixture() });
    await advance(5000);
    expect(screen.getByText('In sync')).toBeInTheDocument();
  });

  it('shows the not-found page for a 404 after the dashboard was shown, and the dashboard when it is back', async () => {
    const { container } = await open({ [DASHBOARD]: { json: kundeA } });
    mock.on(DASHBOARD, { status: 404, json: { error: 'not_found', message: 'no such dashboard' } });
    await advance(5000);
    expect(screen.getByText('There is no dashboard named kunde-a.')).toBeInTheDocument();
    expect(container.querySelector('.wall-panel')).toBeNull();
    mock.on(DASHBOARD, { json: kundeA });
    await advance(5000);
    expect(title()).toBe('Kunde A');
    expect(container.querySelectorAll('.wall-panel')).toHaveLength(4);
  });

  it('says that there is no such dashboard for 404, with a link back to the dashboards', async () => {
    const { container } = await open({}, 'gone');
    expect(screen.getByText('There is no dashboard named gone.')).toBeInTheDocument();
    const back = screen.getByRole('link', { name: 'Back to the dashboards' });
    expect(back).toHaveAttribute('href', '/dashboards');
    // Still the dark full-screen wall, but not a dashboard.
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(container.querySelector('.wall-header')).toBeNull();
    fireEvent.click(back);
    expect(window.location.pathname).toBe('/dashboards');
  });

  it('shows the Out Of Sync band from the status when GitOps is on', async () => {
    const sync = { ...statusFixture().sync, status: 'out_of_sync' as const };
    await open({ [DASHBOARD]: { json: kundeA }, 'GET /status': { json: statusFixture({ sync }) } });
    expect(screen.getByText(/^Out of sync since/)).toBeInTheDocument();
  });

  it('says nothing about the sync state when GitOps is off', async () => {
    const sync = { ...statusFixture().sync, status: 'out_of_sync' as const };
    await open({ [DASHBOARD]: { json: kundeA }, 'GET /status': { json: statusFixture({ gitops: false, sync }) } });
    expect(screen.queryByText(/Out of sync/)).not.toBeInTheDocument();
    expect(screen.queryByText('In sync')).not.toBeInTheDocument();
  });

  it('says it is in sync from the status', async () => {
    await open({ [DASHBOARD]: { json: kundeA } });
    expect(screen.getByText('In sync')).toBeInTheDocument();
  });

  it('leads back to the dashboards on the desk', async () => {
    await open({ [DASHBOARD]: { json: kundeA } });
    expect(screen.getByRole('link', { name: 'Back to the desk' })).toHaveAttribute('href', '/dashboards');
  });

  it("shows the server's time, from the Date header of the answers", async () => {
    const server = new Date(NOW.getTime() + 2 * 60_000).toUTCString();
    await open({ [DASHBOARD]: { json: kundeA }, 'GET /status': { json: statusFixture(), headers: { Date: server } } });
    await advance(5000);
    expect(document.querySelector('.wall-clock')).toHaveTextContent('14:32');
  });

  it('leaves no timer and no request running after unmount, also while the connection is lost', async () => {
    const { unmount } = await open({ [DASHBOARD]: { json: kundeA } });
    mock.on(DASHBOARD, 'network');
    await advance(5000);
    // The polls, the clock, the frame's idle timer and the one for the lost connection.
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    const before = mock.requests.length;
    unmount();
    // Preact runs the cleanups after its next frame (a 35 ms timer).
    await advance(100);
    expect(vi.getTimerCount()).toBe(0);
    await advance(60_000);
    expect(mock.requests).toHaveLength(before);
  });
});

describe('WallPage in the app', () => {
  it('shows nothing of the previous dashboard after navigating from one wall to another', async () => {
    window.history.replaceState(null, '', '/wall/a');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
      'GET /dashboards/a': { json: dashboardDetailFixture({ id: 'a', name: 'Alpha' }) },
      'GET /dashboards/b': () => new Promise(() => {}),
    });
    const { container } = render(<App />);
    await advance(0);
    await advance(0);
    expect(title()).toBe('Alpha');
    navigate('/wall/b');
    await advance(0);
    expect(title()).toBe('b');
    expect(screen.queryByText('Alpha')).not.toBeInTheDocument();
    expect(container.querySelector('.wall-panel')).toBeNull();
    expect(mock.count('GET /dashboards/b')).toBe(1);
  });
});
