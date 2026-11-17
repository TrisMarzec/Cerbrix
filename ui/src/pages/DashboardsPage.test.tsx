import { fireEvent, render, screen, within } from '@testing-library/preact';

import { dashboardFixture, displayFixture } from '../test/fixtures';
import { mockFetch, type FetchMock } from '../test/mockFetch';
import { advance } from '../test/timers';
import { expectStaleAbove, staleNotes } from '../test/stale';
import { DashboardsPage } from './DashboardsPage';

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
});

async function open(routes: Parameters<typeof mockFetch>[0]) {
  mock = mockFetch(routes);
  const view = render(<DashboardsPage />);
  await advance(0);
  return view;
}

function dataRows() {
  return within(screen.getByRole('table', { name: 'Dashboards' })).getAllByRole('row').slice(1);
}

describe('dashboards table', () => {
  it('shows nothing but the heading before the first answer', () => {
    mock = mockFetch({ 'GET /dashboards': { json: { items: [] } }, 'GET /displays': { json: { items: [] } } });
    render(<DashboardsPage />);
    expect(screen.getByRole('heading', { level: 1, name: 'Dashboards' })).toBeInTheDocument();
    expect(screen.queryByText(/No dashboards yet/)).not.toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('keeps the order of the server and shows name, file and priority', async () => {
    await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({ id: 'b', name: 'Bravo', file: 'dashboards/b.json', priority: 5 }),
            dashboardFixture({ id: 'a', name: 'Alpha', file: 'dashboards/a.json', priority: 30 }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    const rows = dataRows();
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('Bravo');
    expect(rows[0]).toHaveTextContent('dashboards/b.json');
    expect(rows[0]).toHaveTextContent('5');
    expect(rows[1]).toHaveTextContent('Alpha');
    expect(rows[1]).toHaveTextContent('30');
  });

  it('draws the squares from the counts, problem classes first', async () => {
    const view = await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              host_count: 6,
              hosts_by_state: { critical: 1, high: 0, warning: 0, info: 0, acknowledged: 2, ok: 3 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    const squares = view.container.querySelectorAll('.square');
    expect(squares).toHaveLength(6);
    expect(squares[0]).toHaveClass('sev-critical');
    expect(squares[1]).toHaveClass('is-acknowledged');
    expect(squares[2]).toHaveClass('is-acknowledged');
    expect(squares[3]).toHaveClass('is-ok');
    expect(squares[5]).toHaveClass('is-ok');
    expect(screen.getByRole('img', { name: '6 hosts, 3 with problems' })).toBeInTheDocument();
  });

  it('caps the squares at 40, problems first, and says how many more', async () => {
    const view = await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              host_count: 48,
              hosts_by_state: { critical: 4, high: 0, warning: 0, info: 0, acknowledged: 0, ok: 44 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    const squares = view.container.querySelectorAll('.square');
    expect(squares).toHaveLength(40);
    expect(view.container.querySelectorAll('.square.sev-critical')).toHaveLength(4);
    expect(screen.getByText('and 8 more')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: '48 hosts, 4 with problems' })).toBeInTheDocument();
  });

  it('says the open problems in words, most severe first, without zeros', async () => {
    await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              open_problems: { critical: 2, high: 0, warning: 1, info: 0 },
              unacknowledged: { critical: 2, high: 0, warning: 1, info: 0 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByText('2 critical, 1 warning')).toHaveClass('sev-critical');
  });

  it('adds that all problems are acknowledged', async () => {
    await open({
      'GET /dashboards': {
        json: { items: [dashboardFixture({ open_problems: { critical: 0, high: 0, warning: 1, info: 0 } })] },
      },
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByText('1 warning (all acknowledged)')).toBeInTheDocument();
  });

  it('says "No problems" in the healthy colour', async () => {
    await open({
      'GET /dashboards': { json: { items: [dashboardFixture()] } },
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByText('No problems')).toHaveClass('is-ok');
  });

  it('links a row to the problems of the dashboard, encoding the id', async () => {
    await open({
      'GET /dashboards': { json: { items: [dashboardFixture({ id: 'kunde a/ü&x', name: 'Kunde A' })] } },
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByRole('link', { name: 'Kunde A' })).toHaveAttribute(
      'href',
      '/problems?dashboard=kunde+a%2F%C3%BC%26x',
    );
  });

  it('shows hostile text as text', async () => {
    const hostile = '<img src=x onerror=alert(1)>';
    const view = await open({
      'GET /dashboards': { json: { items: [dashboardFixture({ name: hostile, file: hostile })] } },
      'GET /displays': { json: { items: [displayFixture({ name: hostile })] } },
    });
    expect(view.container.querySelector('img')).toBeNull();
    expect(screen.getAllByText(hostile).length).toBeGreaterThanOrEqual(2);
  });

  it('says so when there are no dashboards', async () => {
    await open({
      'GET /dashboards': { json: { items: [] } },
      'GET /displays': { json: { items: [] } },
    });
    expect(
      screen.getByText(/No dashboards yet\. Add a file under/, { selector: 'p' }),
    ).toHaveTextContent('No dashboards yet. Add a file under dashboards/ in the configuration repository.');
  });

  it('keeps the table and shows no spinner when a later poll fails', async () => {
    await open({
      'GET /dashboards': [{ json: { items: [dashboardFixture({ name: 'Alpha' })] } }, 'network'],
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByRole('link', { name: 'Alpha' })).toBeInTheDocument();
    await advance(10_000);
    expect(mock.count('GET /dashboards')).toBe(2);
    expect(screen.getByRole('link', { name: 'Alpha' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });

  it('shows a banner and loads again on "Try again" after a failed first load', async () => {
    await open({
      'GET /dashboards': ['network', { json: { items: [dashboardFixture({ name: 'Alpha' })] } }],
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Could not load the dashboards.');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByRole('link', { name: 'Alpha' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('draws no squares and no "more" for 0 hosts', async () => {
    const view = await open({
      'GET /dashboards': { json: { items: [dashboardFixture()] } },
      'GET /displays': { json: { items: [] } },
    });
    expect(view.container.querySelectorAll('.square')).toHaveLength(0);
    expect(screen.queryByText(/more$/)).not.toBeInTheDocument();
    expect(screen.getByRole('img', { name: '0 hosts, none with problems' })).toBeInTheDocument();
  });

  it('counts the hidden hosts from host_count for 5,000 hosts', async () => {
    const view = await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              host_count: 5000,
              hosts_by_state: { critical: 0, high: 0, warning: 0, info: 0, acknowledged: 0, ok: 5000 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    expect(view.container.querySelectorAll('.square')).toHaveLength(40);
    expect(screen.getByText('and 4960 more')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: '5000 hosts, none with problems' })).toBeInTheDocument();
  });

  it('counts the missing hosts as more when the counts sum to less than host_count', async () => {
    const view = await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              host_count: 10,
              hosts_by_state: { critical: 1, high: 0, warning: 0, info: 0, acknowledged: 0, ok: 2 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    expect(view.container.querySelectorAll('.square')).toHaveLength(3);
    expect(screen.getByText('and 7 more')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: '10 hosts, 1 with a problem' })).toBeInTheDocument();
  });

  it('never says a negative number of more hosts', async () => {
    const view = await open({
      'GET /dashboards': {
        json: {
          items: [
            dashboardFixture({
              host_count: 1,
              hosts_by_state: { critical: 0, high: 0, warning: 0, info: 0, acknowledged: 0, ok: 3 },
            }),
          ],
        },
      },
      'GET /displays': { json: { items: [] } },
    });
    expect(view.container.querySelectorAll('.square')).toHaveLength(3);
    expect(screen.queryByText(/more$/)).not.toBeInTheDocument();
  });
});

describe('wall displays', () => {
  const two = [
    dashboardFixture({ id: 'a', name: 'Kunde A – Region 2' }),
    dashboardFixture({ id: 'b', name: 'Kunde B' }),
  ];

  function displayTable() {
    return screen.getByRole('table', { name: 'Wall displays' });
  }

  it('describes a fixed display that never switches', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [displayFixture({ name: 'Office', show: { dashboard: 'a' } })] } },
    });
    const row = within(displayTable()).getAllByRole('row')[1];
    expect(row).toHaveTextContent('Office');
    expect(row).toHaveTextContent('Shows Kunde A – Region 2, never switches');
    expect(row).toHaveTextContent('Following its setting');
  });

  it('describes a rotating display that switches on alarms', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': {
        json: {
          items: [
            displayFixture({
              show: { rotate: ['a', 'b', 'c', 'd'], every: '30s' },
              switch_on_alarm: { min_severity: 'critical' },
            }),
            displayFixture({
              id: 'm',
              show: { rotate: ['a', 'b'], every: '5m' },
              switch_on_alarm: { min_severity: 'warning' },
              dashboards: ['a'],
            }),
            displayFixture({ id: 'h', show: { rotate: ['a', 'b'], every: '1h' } }),
          ],
        },
      },
    });
    const rows = within(displayTable()).getAllByRole('row');
    expect(rows[1]).toHaveTextContent('Rotates through 4 dashboards every 30 s, switches on alarms from Critical');
    expect(rows[2]).toHaveTextContent(
      'Rotates through 2 dashboards every 5 min, switches on alarms from Warning (limited to 1 dashboard)',
    );
    expect(rows[3]).toHaveTextContent('Rotates through 2 dashboards every 1 h, never switches');
  });

  it('names the dashboard shown because of alarms and the waiting ones', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': {
        json: {
          items: [
            displayFixture({
              target: 'b',
              causes: ['p1', 'p2'],
              waiting: ['a', 'unknown-id'],
              switch_on_alarm: { min_severity: 'warning' },
            }),
          ],
        },
      },
    });
    const showing = screen.getByText('Showing Kunde B because of 2 alarms');
    expect(showing).toHaveClass('sev-critical');
    expect(screen.getByText('Waiting: Kunde A – Region 2, unknown-id')).toBeInTheDocument();
  });

  it('uses the singular for one alarm and the id for an unknown dashboard', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [displayFixture({ target: 'gone', causes: ['p1'] })] } },
    });
    expect(screen.getByText('Showing gone because of 1 alarm')).toBeInTheDocument();
  });

  it('says so when there are no displays', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [] } },
    });
    expect(screen.getByRole('heading', { level: 2, name: 'Wall displays' })).toBeInTheDocument();
    expect(screen.getByText('No wall displays configured.')).toBeInTheDocument();
  });

  it('keeps the dashboards when the displays fail, and can try again', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': ['network', { json: { items: [displayFixture({ name: 'Office' })] } }],
    });
    expect(screen.getByRole('link', { name: 'Kunde B' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Could not load the wall displays.');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByText('Office')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('holds the displays table back until the dashboards have answered', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      'GET /dashboards': async () => {
        await gate;
        return { json: { items: two } };
      },
      'GET /displays': {
        json: { items: [displayFixture({ name: 'Office', show: { dashboard: 'a' }, target: 'b', causes: ['p'] })] },
      },
    });
    expect(screen.getByRole('heading', { level: 2, name: 'Wall displays' })).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.queryByText(/Shows a/)).not.toBeInTheDocument();
    release();
    await advance(0);
    const row = within(displayTable()).getAllByRole('row')[1];
    expect(row).toHaveTextContent('Shows Kunde A – Region 2');
    expect(row).toHaveTextContent('Showing Kunde B because of 1 alarm');
  });

  it('shows a sentence instead of ids when the dashboards failed but the displays loaded', async () => {
    await open({
      'GET /dashboards': 'network',
      'GET /displays': { json: { items: [displayFixture({ show: { dashboard: 'a' } })] } },
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Could not load the dashboards.');
    expect(
      screen.getByText('Wall displays are shown once the dashboards could be loaded.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.queryByText(/Shows a/)).not.toBeInTheDocument();
  });

  it('limits the sentence to a list only for a display that switches', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': {
        json: {
          items: [
            displayFixture({ id: 'x', show: { dashboard: 'a' }, switch_on_alarm: { min_severity: 'high' }, dashboards: ['a', 'b'] }),
            displayFixture({ id: 'y', show: { dashboard: 'a' }, dashboards: ['a', 'b'] }),
          ],
        },
      },
    });
    const rows = within(displayTable()).getAllByRole('row');
    expect(rows[1]).toHaveTextContent('Shows Kunde A – Region 2, switches on alarms from High (limited to 2 dashboards)');
    expect(rows[2]).toHaveTextContent(/Shows Kunde A – Region 2, never switches/);
    expect(rows[2]).not.toHaveTextContent('limited');
  });

  it.each([
    ['30s', '30 s'],
    ['90s', '90 s'],
    ['1m30s', '1 min 30 s'],
    ['1h', '1 h'],
    ['1d', '1 d'],
    ['often', 'often'],
    ['5 min', '5 min'],
  ])('renders the rotation period %s as %s', async (every, shown) => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [displayFixture({ show: { rotate: ['a', 'b'], every } })] } },
    });
    expect(within(displayTable()).getAllByRole('row')[1]).toHaveTextContent(
      `Rotates through 2 dashboards every ${shown}, never switches`,
    );
  });

  it('uses the singular for a rotation of one dashboard', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [displayFixture({ show: { rotate: ['a'], every: '30s' } })] } },
    });
    expect(within(displayTable()).getAllByRole('row')[1]).toHaveTextContent(
      'Rotates through 1 dashboard every 30 s',
    );
  });

  it('says "an alarm" for a target without causes', async () => {
    await open({
      'GET /dashboards': { json: { items: two } },
      'GET /displays': { json: { items: [displayFixture({ target: 'b', causes: [] })] } },
    });
    expect(screen.getByText('Showing Kunde B because of an alarm')).toBeInTheDocument();
  });
});

describe('dashboards that are not up to date', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 10, 17, 14, 30, 0));
  });

  it('says since when each list is old when its later poll fails', async () => {
    await open({
      'GET /dashboards': [{ json: { items: [dashboardFixture()] } }, 'network'],
      'GET /displays': [{ json: { items: [displayFixture()] } }, { json: { items: [displayFixture()] } }, 'network'],
    });
    expect(staleNotes()).toHaveLength(0);
    await advance(10_000);
    expect(staleNotes()).toHaveLength(1);
    expectStaleAbove(screen.getByRole('table', { name: 'Dashboards' }), '14:30');
    await advance(10_000);
    expect(staleNotes()).toHaveLength(2);
    expectStaleAbove(screen.getByRole('table', { name: 'Wall displays' }), '14:30');
  });
});
