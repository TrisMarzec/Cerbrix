import { readFileSync } from 'node:fs';

import { act, render, screen, within } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';
import type uPlot from 'uplot';

import type { HostDetail, HostValue, Role, Series } from '../api/types';
import { GRAPH_HEIGHT } from '../components/Graph';
import { SessionProvider } from '../session';
import { NowContext, StatusContext } from '../shell/context';
import {
  acknowledgedProblem,
  criticalProblem,
  hostDetailFixture,
  NOW,
} from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { expectStaleAbove, staleNotes } from '../test/stale';
import { HostPage } from './HostPage';

const plots: { data: uPlot.AlignedData; destroy: ReturnType<typeof vi.fn>; destroyed: () => boolean }[] = [];

vi.mock('uplot', () => ({
  default: class {
    data: uPlot.AlignedData;
    destroy = vi.fn();
    destroyed = () => this.destroy.mock.calls.length > 0;
    setData = vi.fn();
    setSize = vi.fn();
    constructor(_opts: uPlot.Options, data: uPlot.AlignedData) {
      this.data = data;
      plots.push(this);
    }
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  plots.length = 0;
});

afterEach(() => {
  mock.restore();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
});

const T0 = 1_794_865_800_000;

function series(overrides: Partial<Series> = {}): Series {
  return {
    key: 'cpu.usage',
    params: {},
    from: '2026-11-17T08:30:00Z',
    to: '2026-11-17T14:30:00Z',
    step_secs: 90,
    points: [
      [T0, 20],
      [T0 + 90_000, 29.4],
    ],
    thresholds: [],
    ...overrides,
  };
}

function values(items: HostValue[], available = true) {
  return { json: { items, metrics_available: available } };
}

const cpuValue: HostValue = {
  key: 'cpu.usage',
  params: {},
  value: 29.407,
  at: new Date(NOW.getTime() - 20_000).toISOString(),
};

interface Options {
  role?: Role;
  name?: string;
  search?: string;
  detail?: Responder | Responder[];
  values?: Responder | Responder[];
  routes?: Record<string, Responder | Responder[]>;
  reloadStatus?: () => void;
}

async function open(o: Options = {}) {
  const name = o.name ?? 'web-01';
  window.history.replaceState(null, '', `/hosts/${encodeURIComponent(name)}${o.search ?? ''}`);
  const base = `/hosts/${encodeURIComponent(name)}`;
  mock = mockFetch({
    'GET /session': { json: { name: 'anna', role: o.role ?? 'viewer' } },
    [`GET ${base}`]: o.detail ?? { json: hostDetailFixture({ name }) },
    [`GET ${base}/values`]: o.values ?? values([cpuValue]),
    [`GET ${base}/series`]: { json: series() },
    ...o.routes,
  });
  const view = render(
    <SessionProvider>
      <StatusContext.Provider
        value={{ status: undefined, staleSince: undefined, reloadStatus: o.reloadStatus ?? (() => {}) }}
      >
        <NowContext.Provider value={NOW}>
          <HostPage name={name} />
        </NowContext.Provider>
      </StatusContext.Provider>
    </SessionProvider>,
  );
  await advance(0);
  await advance(0);
  await advance(0);
  return view;
}

function seriesRequests() {
  return mock.requests.filter((r) => r.method === 'GET' && r.path.endsWith('/series'));
}

function detailWith(overrides: Partial<HostDetail>) {
  return { json: hostDetailFixture(overrides) };
}

describe('head', () => {
  it('shows the host, its fields and the dashboards it belongs to', async () => {
    await open({
      detail: detailWith({
        dashboards: [
          { id: 'e2e', name: 'E2E' },
          { id: 'prod wall', name: 'Production' },
        ],
      }),
    });
    expect(screen.getByRole('heading', { level: 1, name: 'web-01' })).toBeInTheDocument();
    expect(screen.getByText('10.20.1.21')).toBeInTheDocument();
    expect(screen.getByText('web')).toBeInTheDocument();
    expect(screen.getByText('linux-base')).toBeInTheDocument();
    expect(screen.getByText('hosts/web-01.json')).toBeInTheDocument();
    expect(screen.getByText('0.1.0')).toBeInTheDocument();
    expect(screen.getByText('No problems')).toBeInTheDocument();
    expect(screen.getByText('Managed by Git')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'E2E' })).toHaveAttribute('href', '/problems?dashboard=e2e');
    expect(screen.getByRole('link', { name: 'Production' })).toHaveAttribute(
      'href',
      '/problems?dashboard=prod+wall',
    );
  });

  it('says unknown, not enrolled and created by hand where the host has no such thing', async () => {
    await open({
      detail: detailWith({
        agent_version: null,
        enrolled_at: null,
        source_file: null,
        managed_by_git: false,
        items: [],
        triggers: [],
        templates: [],
      }),
    });
    expect(screen.getByText('unknown')).toBeInTheDocument();
    expect(screen.getByText('not enrolled')).toBeInTheDocument();
    expect(screen.getByText('Created by hand')).toBeInTheDocument();
    expect(screen.queryByText('Managed by Git')).not.toBeInTheDocument();
    expect(screen.getByText('This host has no items configured.')).toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);
  });

  it('shows the number of open problems in the worst severity, with the word', async () => {
    await open({
      detail: detailWith({
        open_problems: [acknowledgedProblem, criticalProblem],
      }),
    });
    const pill = screen.getByText('2 open problems · Critical');
    expect(pill).toHaveClass('sev-critical');
  });

  it('says one open problem in the singular and marks all-acknowledged', async () => {
    await open({ detail: detailWith({ open_problems: [acknowledgedProblem] }) });
    expect(screen.getByText('1 open problem · Warning (acknowledged)')).toBeInTheDocument();
  });

  it('links to the past problems of a host whose name has a dot', async () => {
    await open({ name: 'db.example.com' });
    expect(screen.getByRole('link', { name: 'Past problems of this host' })).toHaveAttribute(
      'href',
      '/problems?state=resolved&host=db.example.com',
    );
  });

  it('shows nothing but the heading before the first answer', async () => {
    window.history.replaceState(null, '', '/hosts/web-01');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /hosts/web-01': () => new Promise(() => {}),
      'GET /hosts/web-01/values': () => new Promise(() => {}),
    });
    render(
      <SessionProvider>
        <NowContext.Provider value={NOW}>
          <HostPage name="web-01" />
        </NowContext.Provider>
      </SessionProvider>,
    );
    await advance(0);
    await advance(0);
    expect(screen.getByRole('heading', { level: 1, name: 'web-01' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText('Open problems')).not.toBeInTheDocument();
  });
});

describe('failures', () => {
  it('offers to try again when the first load fails', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      detail: [{ status: 500, json: { error: 'internal', message: 'boom' } }, { json: hostDetailFixture() }],
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Could not load the host.');
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(50);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByText('Current values')).toBeInTheDocument();
  });

  it('shows the not-found state for an unknown host', async () => {
    await open({
      name: 'nope',
      detail: { status: 404, json: { error: 'not_found', message: 'no host' } },
      values: { status: 404, json: { error: 'not_found', message: 'no host' } },
    });
    expect(screen.getByText('There is no host named nope.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Back to hosts' })).toHaveAttribute('href', '/hosts');
    expect(screen.queryByText('Open problems')).not.toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);
  });

  it('shows a host name from the URL as text', async () => {
    const hostile = '<img src=x onerror=alert(1)>';
    await open({
      name: hostile,
      detail: { status: 404, json: { error: 'not_found', message: 'no host' } },
    });
    expect(screen.getByText(`There is no host named ${hostile}.`)).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
  });

  it('keeps the page when a later poll fails', async () => {
    await open({
      detail: [{ json: hostDetailFixture() }, 'network'],
    });
    await advance(10_000);
    expect(screen.getByText('Current values')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});

describe('roles', () => {
  const withProblem = detailWith({ open_problems: [criticalProblem] });

  it('gives a viewer no Acknowledge, Close or Release', async () => {
    await open({ role: 'viewer', detail: withProblem });
    expect(screen.getByText('Disk almost full mount=/')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Acknowledge/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Close/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Release/ })).not.toBeInTheDocument();
  });

  it('gives an operator Acknowledge and Close but not Release', async () => {
    await open({ role: 'operator', detail: withProblem });
    expect(screen.getByRole('button', { name: /^Acknowledge / })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Close problem / })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Release/ })).not.toBeInTheDocument();
  });

  it('gives an admin Release agent binding', async () => {
    await open({ role: 'admin', detail: withProblem });
    expect(screen.getByRole('button', { name: 'Release agent binding' })).toBeInTheDocument();
  });

  it('offers to acknowledge again when the problem is acknowledged', async () => {
    await open({ role: 'operator', detail: detailWith({ open_problems: [acknowledgedProblem] }) });
    expect(screen.getByRole('button', { name: /^Acknowledge again / })).toBeInTheDocument();
  });
});

describe('actions', () => {
  const path = `/problems/${criticalProblem.id}`;

  it('acknowledges through the dialog and reloads the host', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'operator',
      detail: detailWith({ open_problems: [criticalProblem] }),
      routes: {
        [`POST ${path}/ack`]: {
          json: { ...criticalProblem, acknowledged_at: NOW.toISOString(), acknowledged_by: 'anna' },
        },
      },
    });
    const before = mock.count('GET /hosts/web-01');
    await user.click(screen.getByRole('button', { name: /^Acknowledge / }));
    const dialog = screen.getByRole('dialog');
    await user.type(within(dialog).getByLabelText('Comment (optional)'), 'looking');
    await user.click(within(dialog).getByRole('button', { name: 'Acknowledge' }));
    await advance(0);
    expect(mock.requests.find((r) => r.path === `${path}/ack`)?.body).toEqual({ comment: 'looking' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(mock.count('GET /hosts/web-01')).toBeGreaterThan(before);
  });

  it('closes a problem through the second button', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'operator',
      detail: detailWith({ open_problems: [criticalProblem] }),
      routes: {
        [`POST ${path}/close`]: { json: { ...criticalProblem, resolved_at: NOW.toISOString() } },
      },
    });
    await user.click(screen.getByRole('button', { name: /^Close problem / }));
    const dialog = screen.getByRole('dialog');
    await user.type(within(dialog).getByLabelText('Reason'), 'disk removed');
    await user.click(within(dialog).getByRole('button', { name: 'Close problem' }));
    await advance(0);
    expect(mock.requests.find((r) => r.path === `${path}/close`)?.body).toEqual({
      comment: 'disk removed',
    });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('keeps the dialog open on 409 and reloads the host when it is closed', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'operator',
      detail: [
        { json: hostDetailFixture({ open_problems: [criticalProblem] }) },
        { json: hostDetailFixture({ open_problems: [] }) },
      ],
      routes: { [`POST ${path}/ack`]: { status: 409, json: { error: 'conflict', message: 'x' } } },
    });
    await user.click(screen.getByRole('button', { name: /^Acknowledge / }));
    const dialog = screen.getByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Acknowledge' }));
    await advance(0);
    expect(within(screen.getByRole('dialog')).getByRole('alert')).toHaveTextContent(
      'This problem is already resolved.',
    );
    const before = mock.count('GET /hosts/web-01');
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await advance(0);
    expect(mock.count('GET /hosts/web-01')).toBeGreaterThan(before);
    expect(screen.getByText('No open problems.')).toBeInTheDocument();
  });

  it('asks before releasing, names the host, and says so afterwards', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'admin',
      routes: { 'POST /hosts/web-01/release': { status: 204 } },
    });
    await user.click(screen.getByRole('button', { name: 'Release agent binding' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent('web-01');
    expect(dialog).toHaveTextContent('The agent on this host must enrol again with a new key.');
    expect(mock.requests.some((r) => r.method === 'POST')).toBe(false);
    await user.click(within(dialog).getByRole('button', { name: 'Release binding' }));
    await advance(0);
    expect(mock.count('POST /hosts/web-01/release')).toBe(1);
    expect(screen.getByText('Agent binding released.')).toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('shows a refused release inside its dialog', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'admin',
      routes: { 'POST /hosts/web-01/release': { status: 403, json: { error: 'forbidden', message: 'x' } } },
    });
    await user.click(screen.getByRole('button', { name: 'Release agent binding' }));
    await user.click(screen.getByRole('button', { name: 'Release binding' }));
    await advance(0);
    expect(within(screen.getByRole('dialog')).getByRole('alert')).toHaveTextContent(
      'Your role may not do this.',
    );
  });

  it('shows the not-found state when the host is gone at release', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'admin',
      routes: { 'POST /hosts/web-01/release': { status: 404, json: { error: 'not_found', message: 'x' } } },
    });
    await user.click(screen.getByRole('button', { name: 'Release agent binding' }));
    await user.click(screen.getByRole('button', { name: 'Release binding' }));
    await advance(0);
    expect(screen.getByText('There is no host named web-01.')).toBeInTheDocument();
  });
});

describe('graphs', () => {
  it('requests an item without a parameter with item and range', async () => {
    await open();
    const [request] = seriesRequests();
    expect(seriesRequests()).toHaveLength(1);
    expect(request?.path).toBe('/hosts/web-01/series');
    expect(request?.query.toString()).toBe('item=cpu.usage&range=6h');
    expect(plots).toHaveLength(1);
    expect(
      screen.getByRole('img', { name: 'cpu.usage over the last 6 hours, latest 29.4 %, 0 thresholds' }),
    ).toBeInTheDocument();
  });

  const diskHost = detailWith({
    items: [{ key: 'disk.used_percent', params: null, interval_secs: 300, origin: 'template:linux-base' }],
  });
  const diskValues = values([
    { key: 'disk.used_percent', params: { mount: '/' }, value: 41.5, at: cpuValue.at },
    { key: 'disk.used_percent', params: { mount: 'C:\\' }, value: 90.3, at: cpuValue.at },
  ]);

  it('requests one series per parameter value, sorted', async () => {
    await open({ detail: diskHost, values: diskValues });
    const requests = seriesRequests();
    expect(requests.map((r) => r.query.toString())).toEqual([
      'item=disk.used_percent&range=6h&p_mount=%2F',
      'item=disk.used_percent&range=6h&p_mount=C%3A%5C',
    ]);
    expect(screen.getByRole('heading', { level: 3, name: 'disk.used_percent · mount=/' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 3, name: 'disk.used_percent · mount=C:\\' })).toBeInTheDocument();
  });

  it('says so when an item with a parameter has no value yet, and draws no graph for it', async () => {
    await open({
      detail: detailWith({
        items: [{ key: 'disk.used_percent', params: { mount: '/' }, interval_secs: 300, origin: 'host' }],
      }),
      values: values([]),
    });
    expect(screen.getByText('No values yet for disk.used_percent')).toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);
  });

  it('draws the thresholds the series answer carries', async () => {
    await open({
      routes: {
        'GET /hosts/web-01/series': {
          json: series({
            thresholds: [{ trigger: 'CPU high', severity: 'high', op: '>', value: 90 }],
          }),
        },
      },
    });
    expect(screen.getByText(/CPU high: > 90.0 %/)).toBeInTheDocument();
    expect(screen.getByRole('img', { name: /1 threshold$/ })).toBeInTheDocument();
  });

  it('changes the range in the URL and in every request', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({ detail: diskHost, values: diskValues });
    expect(screen.getByRole('button', { name: '6 h' })).toHaveAttribute('aria-pressed', 'true');
    await user.click(screen.getByRole('button', { name: '24 h' }));
    await advance(0);
    expect(window.location.search).toBe('?range=24h');
    expect(screen.getByRole('button', { name: '24 h' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: '6 h' })).toHaveAttribute('aria-pressed', 'false');
    const latest = seriesRequests().slice(-2);
    expect(latest.map((r) => r.query.get('range'))).toEqual(['24h', '24h']);
    await user.click(screen.getByRole('button', { name: '6 h' }));
    await advance(0);
    expect(window.location.search).toBe('');
  });

  it.each([['?range=garbage'], ['?range='], ['?range=7D']])('falls back to 6 h for %s', async (search) => {
    await open({ search });
    expect(seriesRequests()[0]?.query.get('range')).toBe('6h');
    expect(screen.getByRole('button', { name: '6 h' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('takes 7 d from the URL', async () => {
    await open({ search: '?range=7d' });
    expect(seriesRequests()[0]?.query.get('range')).toBe('7d');
  });

  it('shows the sentence in place of one graph when its series answers 503', async () => {
    await open({
      detail: diskHost,
      values: diskValues,
      routes: {
        'GET /hosts/web-01/series': (request) =>
          request.query.get('p_mount') === '/'
            ? { status: 503, json: { error: 'metrics_unavailable', message: 'down' } }
            : { json: series({ key: 'disk.used_percent', params: { mount: 'C:\\' } }) },
      },
    });
    expect(screen.getByText('The metrics store did not answer')).toBeInTheDocument();
    expect(plots).toHaveLength(1);
    expect(screen.getByText('Current values')).toBeInTheDocument();
  });

  it('says there is no data for an empty series', async () => {
    await open({ routes: { 'GET /hosts/web-01/series': { json: series({ points: [] }) } } });
    expect(screen.getByText('No data in this range')).toBeInTheDocument();
    expect(plots).toHaveLength(0);
  });

  it('offers to try again when a series cannot be loaded', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      routes: {
        'GET /hosts/web-01/series': [
          { status: 500, json: { error: 'internal', message: 'boom' } },
          { json: series() },
        ],
      },
    });
    expect(screen.getByText(/Could not load this graph: boom/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(50);
    expect(plots).toHaveLength(1);
  });

  it('reloads each series every 60 seconds', async () => {
    await open();
    expect(seriesRequests()).toHaveLength(1);
    await advance(30_000);
    expect(seriesRequests()).toHaveLength(1);
    await advance(30_000);
    expect(seriesRequests()).toHaveLength(2);
  });
});

describe('graphs below the fold', () => {
  type Callback = (entries: { isIntersecting: boolean; target: Element }[]) => void;
  const observers: { callback: Callback; targets: Element[]; disconnected: boolean }[] = [];

  beforeEach(() => {
    observers.length = 0;
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        callback: Callback;
        targets: Element[] = [];
        disconnected = false;
        constructor(callback: Callback) {
          this.callback = callback;
          observers.push(this);
        }
        observe(el: Element) {
          this.targets.push(el);
        }
        disconnect() {
          this.disconnected = true;
        }
      },
    );
  });

  const twoItems = detailWith({
    items: [
      { key: 'cpu.usage', params: null, interval_secs: 5, origin: 'host' },
      { key: 'memory.used_percent', params: null, interval_secs: 5, origin: 'host' },
    ],
  });

  it('requests a series only after its graph is reported visible', async () => {
    await open({ detail: twoItems });
    expect(screen.getByRole('heading', { level: 3, name: 'cpu.usage' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 3, name: 'memory.used_percent' })).toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);

    const second = observers.find((o) =>
      o.targets.some((t) => t.querySelector('h3')?.textContent === 'memory.used_percent'),
    );
    expect(second).toBeDefined();
    await act(async () => {
      second?.callback([{ isIntersecting: false, target: second.targets[0] as Element }]);
    });
    await advance(0);
    expect(seriesRequests()).toHaveLength(0);

    await act(async () => {
      second?.callback([{ isIntersecting: true, target: second.targets[0] as Element }]);
    });
    await advance(0);
    expect(seriesRequests().map((r) => r.query.get('item'))).toEqual(['memory.used_percent']);
    expect(second?.disconnected).toBe(true);
  });
});

describe('current values and triggers', () => {
  it('shows the values with item, value and age', async () => {
    await open({
      values: values([
        cpuValue,
        { key: 'disk.used_percent', params: { mount: '/' }, value: 41.5, at: cpuValue.at },
        { key: 'memory.used', params: {}, value: 2048, at: cpuValue.at },
      ]),
    });
    const table = screen.getByRole('table', { name: 'Current values' });
    expect(table.parentElement).toHaveClass('table-scroll');
    const rows = within(table).getAllByRole('row').slice(1);
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent('cpu.usage29.4 %20 s ago');
    expect(rows[1]).toHaveTextContent('disk.used_percent · mount=/41.5 %');
    expect(rows[2]).toHaveTextContent('2.0 KiB');
  });

  it('says the values are not available instead of showing a table', async () => {
    await open({ values: values([], false) });
    expect(screen.getByText('Current values are not available right now.')).toBeInTheDocument();
    expect(screen.queryByRole('table', { name: 'Current values' })).not.toBeInTheDocument();
  });

  it('shows the triggers with severity word, origin and state', async () => {
    await open({
      detail: detailWith({
        triggers: [
          {
            name: 'Agent silent',
            expression: 'no data for 1m',
            severity: 'critical',
            origin: 'host',
            firing: true,
          },
          {
            name: 'CPU high',
            expression: 'cpu.usage > 90 for 5m',
            severity: 'high',
            origin: 'template:linux-base',
            firing: false,
          },
        ],
      }),
    });
    expect(screen.getByRole('table', { name: 'Triggers' }).parentElement).toHaveClass('table-scroll');
    const rows = within(screen.getByRole('table', { name: 'Triggers' })).getAllByRole('row').slice(1);
    expect(rows[0]).toHaveTextContent('Agent silentno data for 1mCriticalhostFiring');
    expect(within(rows[0] as HTMLElement).getByText('Firing')).toHaveClass('sev-critical');
    expect(rows[1]).toHaveTextContent('CPU highcpu.usage > 90 for 5mHightemplate: linux-baseQuiet');
  });

  it('shows text from the API as text', async () => {
    const hostile = '<img src=x onerror=alert(1)>';
    await open({
      role: 'operator',
      detail: detailWith({
        groups: [hostile],
        triggers: [
          { name: hostile, expression: hostile, severity: 'info', origin: 'host', firing: false },
        ],
        open_problems: [{ ...criticalProblem, summary: hostile, trigger: hostile }],
      }),
    });
    expect(document.querySelector('img')).toBeNull();
    expect(screen.getAllByText(hostile).length).toBeGreaterThanOrEqual(3);
  });
});

describe('values failures', () => {
  const diskHost = detailWith({
    items: [{ key: 'disk.used_percent', params: null, interval_secs: 300, origin: 'host' }],
  });
  const diskValues = values([
    { key: 'disk.used_percent', params: { mount: '/' }, value: 41.5, at: cpuValue.at },
    { key: 'disk.used_percent', params: { mount: '/data' }, value: 10, at: cpuValue.at },
  ]);

  it('shows a banner and requests no graph when the first values load fails', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      detail: diskHost,
      values: [{ status: 500, json: { error: 'internal', message: 'boom' } }, diskValues],
    });
    expect(screen.getByText(/Could not load the current values\./)).toBeInTheDocument();
    expect(
      screen.getByText('Graphs are shown once the current values could be loaded.'),
    ).toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(50);
    expect(seriesRequests()).toHaveLength(2);
  });

  it('shows no graphs without metrics and says the values are not available', async () => {
    await open({ detail: diskHost, values: values([], false) });
    expect(screen.getByText('Current values are not available right now.')).toBeInTheDocument();
    expect(
      screen.getByText('Graphs are shown once the current values could be loaded.'),
    ).toBeInTheDocument();
    expect(seriesRequests()).toHaveLength(0);
    expect(screen.queryByText(/No values yet/)).not.toBeInTheDocument();
  });

  it('keeps the graphs through a poll without metrics and a failed poll, and after recovery', async () => {
    await open({
      detail: diskHost,
      values: [diskValues, values([], false), 'network', diskValues],
    });
    expect(plots).toHaveLength(2);
    const requests = seriesRequests().length;
    await advance(10_000);
    expect(screen.getByText('Current values are not available right now.')).toBeInTheDocument();
    await advance(10_000);
    expect(screen.getAllByRole('heading', { level: 3 })).toHaveLength(2);
    expect(plots).toHaveLength(2);
    expect(plots.every((p) => !p.destroyed())).toBe(true);
    expect(seriesRequests()).toHaveLength(requests);
    await advance(10_000);
    expect(screen.getByRole('table', { name: 'Current values' })).toBeInTheDocument();
    expect(plots).toHaveLength(2);
    expect(plots.every((p) => !p.destroyed())).toBe(true);
    expect(seriesRequests()).toHaveLength(requests);
  });
});

describe('series errors', () => {
  it('says there are no values yet for an item asked for without parameters that has none', async () => {
    await open({
      detail: detailWith({
        items: [{ key: 'disk.used_percent', params: null, interval_secs: 300, origin: 'host' }],
      }),
      values: values([cpuValue]),
      routes: {
        'GET /hosts/web-01/series': {
          status: 400,
          json: { error: 'invalid', message: 'disk.used_percent needs the parameter p_mount' },
        },
      },
    });
    expect(screen.getByText('No values yet for disk.used_percent')).toBeInTheDocument();
  });

  it('shows the server message for any other 400, with Try again', async () => {
    await open({
      routes: {
        'GET /hosts/web-01/series': {
          status: 400,
          json: { error: 'invalid', message: 'unknown parameter p_x' },
        },
      },
    });
    expect(screen.getByText(/Could not load this graph: unknown parameter p_x/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(screen.queryByText(/No values yet/)).not.toBeInTheDocument();
  });

  it('keeps the metrics store sentence for 503', async () => {
    await open({
      routes: {
        'GET /hosts/web-01/series': {
          status: 503,
          json: { error: 'metrics_unavailable', message: 'x' },
        },
      },
    });
    expect(screen.getByText('The metrics store did not answer')).toBeInTheDocument();
    expect(screen.queryByText(/Could not load this graph/)).not.toBeInTheDocument();
  });

  it.each([
    ['database_unavailable', 'The server cannot reach its database.'],
    ['timeout', 'The server took too long to answer.'],
  ])('does not blame the metrics store for a 503 %s', async (code, sentence) => {
    await open({
      routes: {
        'GET /hosts/web-01/series': { status: 503, json: { error: code, message: 'x' } },
      },
    });
    expect(screen.getByText(`Could not load this graph: ${sentence}`)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(screen.queryByText('The metrics store did not answer')).not.toBeInTheDocument();
  });

  it('says when a graph was last updated after a poll failed, and keeps its picture', async () => {
    await open({
      routes: {
        'GET /hosts/web-01/series': [{ json: series() }, 'network'],
      },
    });
    expect(screen.queryByText(/Not updated since/)).not.toBeInTheDocument();
    await advance(60_000);
    expect(screen.getByText(/^Not updated since/)).toHaveTextContent(/^Not updated since \d\d:\d\d$/);
    expect(plots).toHaveLength(1);
  });
});

describe('graph identity and frames', () => {
  it('keeps series apart whose parameters read the same as text', async () => {
    await open({
      detail: detailWith({
        items: [{ key: 'net.rx', params: null, interval_secs: 5, origin: 'host' }],
      }),
      values: values([
        { key: 'net.rx', params: { a: '1, b=2' }, value: 1, at: cpuValue.at },
        { key: 'net.rx', params: { a: '1', b: '2' }, value: 2, at: cpuValue.at },
      ]),
    });
    const requests = seriesRequests();
    expect(requests).toHaveLength(2);
    expect(requests.map((r) => r.query.toString()).sort()).toEqual([
      'item=net.rx&range=6h&p_a=1%2C+b%3D2',
      'item=net.rx&range=6h&p_a=1&p_b=2',
    ]);
  });

  it('gives every frame the class that reserves the height of a loaded graph', async () => {
    await open();
    expect(document.querySelector('section.graph-frame')).not.toBeNull();
    // The literal in HostPage.css follows GRAPH_HEIGHT only through this test (comments in both files).
    const hostCss = readFileSync(`${import.meta.dirname}/HostPage.css`, 'utf8');
    const rule =/\n\.graph-frame\s*\{([^}]*)\}/.exec(hostCss)?.[1] ?? '';
    const height = Number(/min-height:\s*(\d+)px/.exec(rule)?.[1]);
    // The plot plus the title and the readout line.
    expect(height).toBeGreaterThanOrEqual(GRAPH_HEIGHT + 40);
  });
});

describe('optimistic actions', () => {
  function pendingReload(first: HostDetail) {
    let calls = 0;
    return () => {
      calls += 1;
      return calls === 1 ? { json: first } : new Promise<never>(() => {});
    };
  }

  it('shows the row acknowledged before the reload answers', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'operator',
      detail: pendingReload(hostDetailFixture({ open_problems: [criticalProblem] })),
      routes: {
        [`POST /problems/${criticalProblem.id}/ack`]: {
          json: { ...criticalProblem, acknowledged_at: NOW.toISOString(), acknowledged_by: 'anna' },
        },
      },
    });
    await user.click(screen.getByRole('button', { name: /^Acknowledge / }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Acknowledge' }));
    await advance(0);
    expect(screen.getByText(/acknowledged by anna/)).toBeInTheDocument();
    expect(screen.getByText('1 open problem · Critical (acknowledged)')).toBeInTheDocument();
    expect(screen.getByText('1 open problem · Critical (acknowledged)')).toHaveClass('is-acknowledged');
    expect(screen.getByRole('status')).toHaveTextContent('Problem acknowledged.');
  });

  it('removes a closed problem at once', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'operator',
      detail: pendingReload(hostDetailFixture({ open_problems: [criticalProblem] })),
      routes: {
        [`POST /problems/${criticalProblem.id}/close`]: {
          json: { ...criticalProblem, resolved_at: NOW.toISOString() },
        },
      },
    });
    await user.click(screen.getByRole('button', { name: /^Close problem / }));
    await user.type(screen.getByLabelText('Reason'), 'gone');
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Close problem' }));
    await advance(0);
    expect(screen.getByText('No open problems.')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Problem closed.');
  });

  it('asks for the status at once after an action, so the badge follows', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const reloadStatus = vi.fn();
    await open({
      role: 'operator',
      reloadStatus,
      detail: pendingReload(hostDetailFixture({ open_problems: [criticalProblem] })),
      routes: {
        [`POST /problems/${criticalProblem.id}/ack`]: {
          json: { ...criticalProblem, acknowledged_at: NOW.toISOString(), acknowledged_by: 'anna' },
        },
      },
    });
    await user.click(screen.getByRole('button', { name: /^Acknowledge / }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Acknowledge' }));
    await advance(0);
    expect(reloadStatus).toHaveBeenCalledTimes(1);
  });
});

describe('host facts', () => {
  it('writes the enrolment time with the full time as a tooltip', async () => {
    await open();
    const enrolled = new Date(2026, 10, 16, 21, 52).toISOString();
    const time = screen.getByText('16 Nov 21:52');
    expect(time.tagName).toBe('TIME');
    expect(time).toHaveAttribute('title', enrolled);
  });
});

describe('release', () => {
  it('removes the banner when the next action starts', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({ role: 'admin', routes: { 'POST /hosts/web-01/release': { status: 204 } } });
    await user.click(screen.getByRole('button', { name: 'Release agent binding' }));
    await user.click(screen.getByRole('button', { name: 'Release binding' }));
    await advance(0);
    expect(screen.getByText('Agent binding released.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '24 h' }));
    await advance(0);
    expect(screen.queryByText('Agent binding released.')).not.toBeInTheDocument();
  });

  it('cannot be dismissed while the request runs', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await open({
      role: 'admin',
      routes: { 'POST /hosts/web-01/release': () => new Promise<never>(() => {}) },
    });
    await user.click(screen.getByRole('button', { name: 'Release agent binding' }));
    await user.click(screen.getByRole('button', { name: 'Release binding' }));
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    await user.keyboard('{Escape}');
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });
});

describe('host data that is not up to date', () => {
  it('says since when the host is old when a later poll fails', async () => {
    await open({ detail: [{ json: hostDetailFixture({ name: 'web-01' }) }, 'network'] });
    expect(staleNotes()).toHaveLength(0);
    await advance(10_000);
    const head = document.querySelector<HTMLElement>('.host-head');
    if (head === null) throw new Error('no host head');
    expectStaleAbove(head, '14:30');
  });

  it('says since when the current values are old when a later poll fails', async () => {
    await open({ values: [values([cpuValue]), 'network'] });
    await advance(10_000);
    expectStaleAbove(screen.getByRole('table', { name: 'Current values' }), '14:30');
  });

  it('puts the note of a graph above the graph', async () => {
    await open({ routes: { 'GET /hosts/web-01/series': [{ json: series() }, 'network'] } });
    await advance(60_000);
    expectStaleAbove(screen.getByRole('img', { name: /cpu\.usage/ }), '14:31');
  });
});
