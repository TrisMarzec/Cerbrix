import { act, fireEvent, render, screen, within } from '@testing-library/preact';

import type { HostRow } from '../api/types';
import { NowContext } from '../shell/context';
import {
  criticalState,
  hostRowFixture,
  NOW,
  noDataState,
  warningAcknowledgedState,
} from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { expectStaleAbove, staleNotes } from '../test/stale';
import { HostsPage } from './HostsPage';

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
});

function hostsAnswer(
  items: HostRow[],
  extra: { total?: number; metrics_available?: boolean } = {},
) {
  return {
    json: {
      items,
      total: extra.total ?? items.length,
      metrics_available: extra.metrics_available ?? true,
    },
  };
}

async function open(
  url: string,
  hosts: Responder | Responder[],
  groups: Responder = { json: { items: ['db', 'web'] } },
) {
  window.history.replaceState(null, '', url);
  mock = mockFetch({ 'GET /hosts': hosts, 'GET /host-groups': groups });
  const view = render(
    <NowContext.Provider value={NOW}>
      <HostsPage />
    </NowContext.Provider>,
  );
  await advance(0);
  return view;
}

function hostRequests() {
  return mock.requests.filter((r) => r.method === 'GET' && r.path === '/hosts');
}

function lastHostQuery() {
  return hostRequests().at(-1)?.query ?? new URLSearchParams();
}

function region(): HTMLElement {
  const el = document.querySelector<HTMLElement>('.hosts-result');
  if (el === null) throw new Error('no result region');
  return el;
}

/** An answer that waits until release() is called. */
function held(answer: ReturnType<typeof hostsAnswer>) {
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const responder: Responder = async () => {
    await gate;
    return answer;
  };
  return { responder, release: () => release() };
}

function rowAt(i: number): HTMLElement {
  const row = dataRows()[i];
  if (row === undefined) throw new Error(`no row ${i}`);
  return row;
}

function dataRows(): HTMLElement[] {
  return within(screen.getByRole('table', { name: 'Hosts' }))
    .getAllByRole('row')
    .slice(1);
}

describe('request', () => {
  it('sends the filters of the URL and a fixed limit', async () => {
    await open(
      '/hosts?search=db&group=web&state=no_data&offset=100',
      hostsAnswer([hostRowFixture()], { total: 120 }),
    );
    const q = lastHostQuery();
    expect(q.get('search')).toBe('db');
    expect(q.get('group')).toBe('web');
    expect(q.get('state')).toBe('no_data');
    expect(q.get('offset')).toBe('100');
    expect(q.get('limit')).toBe('50');
  });

  it('sends only the limit and offset 0 without filters', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    const q = lastHostQuery();
    expect([...q.keys()].sort()).toEqual(['limit', 'offset']);
    expect(q.get('offset')).toBe('0');
  });

  it('falls back on garbage in the URL', async () => {
    await open('/hosts?state=bogus&offset=-5', hostsAnswer([hostRowFixture()]));
    expect(lastHostQuery().has('state')).toBe(false);
    expect(lastHostQuery().get('offset')).toBe('0');
    await open('/hosts?offset=abc', hostsAnswer([hostRowFixture()]));
    expect(lastHostQuery().get('offset')).toBe('0');
  });

  it('cuts a search longer than 200 characters', async () => {
    await open(`/hosts?search=${'a'.repeat(250)}`, hostsAnswer([hostRowFixture()]));
    expect(lastHostQuery().get('search')).toBe('a'.repeat(200));
  });

  it('sends a group that the list does not know and shows it as selected', async () => {
    await open('/hosts?group=gone', hostsAnswer([], { total: 0 }));
    expect(lastHostQuery().get('group')).toBe('gone');
    expect(screen.getByRole('combobox', { name: 'Group' })).toHaveValue('gone');
    expect(screen.getByText('No host matches.')).toBeInTheDocument();
  });
});

describe('search, group and state', () => {
  it('waits 300 ms after the last key stroke, then updates the URL and resets the offset', async () => {
    await open('/hosts?offset=50', hostsAnswer([hostRowFixture()], { total: 80 }));
    const input = screen.getByRole('searchbox', { name: 'Search' });
    fireEvent.input(input, { target: { value: 'd' } });
    await advance(200);
    fireEvent.input(input, { target: { value: 'db' } });
    await advance(299);
    expect(window.location.search).toBe('?offset=50');
    expect(hostRequests()).toHaveLength(1);
    await advance(1);
    expect(window.location.search).toBe('?search=db');
    expect(lastHostQuery().get('search')).toBe('db');
    expect(lastHostQuery().get('offset')).toBe('0');
  });

  it('applies the search at once on Enter', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    const input = screen.getByRole('searchbox', { name: 'Search' });
    fireEvent.input(input, { target: { value: 'web' } });
    fireEvent.submit(input);
    await advance(0);
    expect(lastHostQuery().get('search')).toBe('web');
  });

  it('clears the search with its button', async () => {
    await open('/hosts?search=web', hostsAnswer([hostRowFixture()]));
    expect(screen.getByRole('searchbox', { name: 'Search' })).toHaveValue('web');
    fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    await advance(0);
    expect(window.location.search).toBe('');
    expect(screen.queryByRole('button', { name: 'Clear search' })).not.toBeInTheDocument();
    expect(lastHostQuery().has('search')).toBe(false);
  });

  it('follows the URL when it changes from outside', async () => {
    await open('/hosts?search=web', hostsAnswer([hostRowFixture()]));
    window.history.replaceState(null, '', '/hosts?search=db');
    await act(async () => {
      window.dispatchEvent(new PopStateEvent('popstate'));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole('searchbox', { name: 'Search' })).toHaveValue('db');
    expect(lastHostQuery().get('search')).toBe('db');
  });

  it('offers All groups and the groups, and filters by one', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    const select = screen.getByRole('combobox', { name: 'Group' });
    expect(
      within(select)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['All groups', 'db', 'web']);
    fireEvent.change(select, { target: { value: 'db' } });
    await advance(0);
    expect(window.location.search).toBe('?group=db');
    expect(lastHostQuery().get('group')).toBe('db');
    fireEvent.change(select, { target: { value: '' } });
    await advance(0);
    expect(lastHostQuery().has('group')).toBe(false);
  });

  it('filters by state', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    const select = screen.getByRole('combobox', { name: 'State' });
    expect(
      within(select)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['All', 'With problems', 'No data', 'Without problems']);
    fireEvent.change(select, { target: { value: 'ok' } });
    await advance(0);
    expect(lastHostQuery().get('state')).toBe('ok');
  });

  it('still works when the groups cannot be loaded', async () => {
    await open('/hosts?group=db', hostsAnswer([hostRowFixture()]), {
      status: 500,
      json: {},
    });
    const select = screen.getByRole('combobox', { name: 'Group' });
    expect(
      within(select)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['All groups', 'db']);
    expect(screen.getByRole('table', { name: 'Hosts' })).toBeInTheDocument();
  });
});

describe('table', () => {
  it('shows nothing but the heading and the controls before the first answer', () => {
    mock = mockFetch({
      'GET /hosts': hostsAnswer([]),
      'GET /host-groups': { json: { items: [] } },
    });
    render(<HostsPage />);
    expect(screen.getByRole('heading', { level: 1, name: 'Hosts' })).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.queryByText('No hosts yet.')).not.toBeInTheDocument();
  });

  it('shows the columns for each kind of host', async () => {
    await open(
      '/hosts',
      hostsAnswer([
        hostRowFixture({
          name: 'db-02.example',
          address: '10.20.1.12',
          groups: ['db', 'region-1'],
          state: criticalState,
          cpu: 34,
          memory: 71,
        }),
        hostRowFixture({
          name: 'edge-01',
          address: null,
          groups: [],
          state: warningAcknowledgedState,
        }),
        hostRowFixture({ name: 'app-01' }),
        hostRowFixture({
          name: 'new-01',
          last_seen_at: null,
          cpu: null,
          memory: null,
          state: noDataState,
        }),
      ]),
    );
    const [crit, acked, healthy, never] = [0, 1, 2, 3].map(rowAt) as [
      HTMLElement,
      HTMLElement,
      HTMLElement,
      HTMLElement,
    ];
    expect(crit).toHaveClass('sev-critical');
    expect(crit).not.toHaveClass('is-acknowledged');
    expect(crit).toHaveTextContent('db-02.example');
    expect(crit).toHaveTextContent('10.20.1.12');
    expect(crit).toHaveTextContent('db, region-1');
    expect(crit).toHaveTextContent('Critical: Disk almost full');
    expect(crit).toHaveTextContent('34.0 %');
    expect(crit).toHaveTextContent('71.0 %');
    expect(crit).toHaveTextContent('10 s ago');

    expect(acked).toHaveClass('sev-warning', 'is-acknowledged');
    expect(acked).toHaveTextContent('Warning: Memory high (acknowledged)');
    expect(acked.querySelector('small')).toBeNull();

    expect(healthy).not.toHaveClass('is-problem');
    expect(healthy).toHaveTextContent('No problems');
    expect(healthy).toHaveTextContent('41.2 %');
    expect(healthy).toHaveTextContent('58.0 %');

    expect(never).toHaveTextContent('never');
    expect(never).toHaveTextContent('Critical: no data for 5m');
    expect(within(never).getAllByText('–')).toHaveLength(2);
  });

  it('shows a dash for cpu null', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture({ cpu: null })]));
    const cells = within(dataRows()[0] as HTMLElement).getAllByRole('cell');
    expect(cells[3]).toHaveTextContent('–');
    expect(cells[4]).toHaveTextContent('58.0 %');
  });

  it('links a row to the host page', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture({ name: 'db-02.example' })]));
    expect(screen.getByRole('link', { name: 'db-02.example' })).toHaveAttribute(
      'href',
      '/hosts/db-02.example',
    );
  });

  it('encodes the host name in the link', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture({ name: 'a/b c' })]));
    expect(screen.getByRole('link', { name: 'a/b c' })).toHaveAttribute('href', '/hosts/a%2Fb%20c');
  });

  it('counts the hosts under the table', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()], { total: 224 }));
    expect(screen.getByText('224 hosts')).toBeInTheDocument();
  });

  it('says 1 host for one host', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    expect(screen.getByText('1 host')).toBeInTheDocument();
  });

  it('shows host text as text', async () => {
    const hostile = '<img src=x onerror=alert(1)>';
    const view = await open(
      '/hosts',
      hostsAnswer([hostRowFixture({ name: hostile, address: hostile, groups: [hostile] })]),
    );
    expect(view.container.querySelector('img')).toBeNull();
    expect(screen.getAllByText(hostile).length).toBeGreaterThanOrEqual(3);
  });

  it('pages with the pager and keeps the other filters', async () => {
    await open('/hosts?state=ok', hostsAnswer([hostRowFixture()], { total: 120 }));
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await advance(0);
    expect(window.location.search).toBe('?state=ok&offset=50');
    expect(lastHostQuery().get('offset')).toBe('50');
    expect(lastHostQuery().get('state')).toBe('ok');
  });

  it('keeps the pager on the shown page and disabled while the next page loads', async () => {
    const next = held(hostsAnswer([hostRowFixture({ name: 'page-2' })], { total: 120 }));
    await open('/hosts', [hostsAnswer([hostRowFixture()], { total: 120 }), next.responder]);
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await advance(0);
    expect(screen.getByText('1–50 of 120')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
    await act(async () => {
      next.release();
      await Promise.resolve();
    });
    await advance(0);
    expect(screen.getByText('51–100 of 120')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Previous' })).toBeEnabled();
  });
});

describe('banner and empty states', () => {
  it('warns when the current values are not available', async () => {
    await open(
      '/hosts',
      hostsAnswer([hostRowFixture({ cpu: null, memory: null })], {
        metrics_available: false,
      }),
    );
    expect(screen.getByText('Current values are not available right now.')).toBeInTheDocument();
    expect(screen.getByRole('table', { name: 'Hosts' })).toBeInTheDocument();
  });

  it('says No host matches. for a search without result', async () => {
    await open('/hosts?search=zzz', hostsAnswer([], { total: 0 }));
    expect(screen.getByText('No host matches.')).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('says No hosts yet. without hosts and without filters', async () => {
    await open('/hosts', hostsAnswer([], { total: 0 }));
    expect(screen.getByText('No hosts yet.')).toBeInTheDocument();
  });
});

describe('failure and change of query', () => {
  it('shows a banner with Try again after a failed first load', async () => {
    await open('/hosts', [
      { status: 500, json: { error: 'internal', message: 'boom' } },
      hostsAnswer([hostRowFixture()]),
    ]);
    expect(screen.getByText(/Could not load the hosts\./)).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByRole('table', { name: 'Hosts' })).toBeInTheDocument();
    expect(screen.queryByText(/Could not load the hosts\./)).not.toBeInTheDocument();
  });

  it('keeps the rows when a later poll fails', async () => {
    await open('/hosts', [hostsAnswer([hostRowFixture({ name: 'kept-01' })]), 'network']);
    await advance(10_000);
    expect(screen.getByText('kept-01')).toBeInTheDocument();
    expect(screen.queryByText(/Could not load the hosts\./)).not.toBeInTheDocument();
  });

  it('dims the old rows until the answer for a new filter arrives', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open('/hosts', [
      hostsAnswer([hostRowFixture({ name: 'old-01' })]),
      async () => {
        await gate;
        return hostsAnswer([hostRowFixture({ name: 'new-01' })]);
      },
    ]);
    expect(region()).toHaveAttribute('aria-busy', 'false');
    fireEvent.change(screen.getByRole('combobox', { name: 'State' }), {
      target: { value: 'problems' },
    });
    await advance(0);
    expect(region()).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByText('old-01')).toBeInTheDocument();
    release();
    await advance(0);
    expect(screen.getByText('new-01')).toBeInTheDocument();
    expect(region()).toHaveAttribute('aria-busy', 'false');
  });

  it('shows the failure, not the old rows, when the first answer for a new filter fails', async () => {
    await open('/hosts', [
      hostsAnswer([hostRowFixture({ name: 'old-01' })]),
      { status: 500, json: {} },
    ]);
    fireEvent.change(screen.getByRole('combobox', { name: 'State' }), {
      target: { value: 'ok' },
    });
    await advance(0);
    expect(screen.getByText(/Could not load the hosts\./)).toBeInTheDocument();
    expect(screen.queryByText('old-01')).not.toBeInTheDocument();
  });
});

describe('offset beyond the last page', () => {
  it('goes back to the last existing page with one more request', async () => {
    await open('/hosts?offset=50', (r) =>
      r.query.get('offset') === '0'
        ? hostsAnswer([hostRowFixture({ name: 'first-01' })], { total: 40 })
        : hostsAnswer([], { total: 40 }),
    );
    await advance(0);
    expect(hostRequests().map((r) => r.query.get('offset'))).toEqual(['50', '0']);
    expect(window.location.search).toBe('');
    expect(screen.getByText('first-01')).toBeInTheDocument();
    expect(screen.queryByText('No host matches.')).not.toBeInTheDocument();
    expect(screen.queryByText('No hosts yet.')).not.toBeInTheDocument();
  });

  it('goes to the start of the last page, not to the first', async () => {
    await open('/hosts?offset=200', (r) =>
      r.query.get('offset') === '200'
        ? hostsAnswer([], { total: 120 })
        : hostsAnswer([hostRowFixture()], { total: 120 }),
    );
    await advance(0);
    expect(hostRequests().map((r) => r.query.get('offset'))).toEqual(['200', '100']);
    expect(window.location.search).toBe('?offset=100');
  });
});

describe('while the answer for a new query is outstanding', () => {
  it('hides the empty sentence and shows the next rows when they arrive', async () => {
    const next = held(hostsAnswer([hostRowFixture({ name: 'new-01' })]));
    await open('/hosts?search=zzz', [hostsAnswer([], { total: 0 }), next.responder]);
    expect(screen.getByText('No host matches.')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'State' }), { target: { value: 'ok' } });
    await advance(0);
    expect(screen.queryByText('No host matches.')).not.toBeInTheDocument();
    expect(screen.queryByText('No hosts yet.')).not.toBeInTheDocument();
    expect(region()).toHaveAttribute('aria-busy', 'true');
    next.release();
    await advance(0);
    expect(screen.getByText('new-01')).toBeInTheDocument();
    expect(region()).toHaveAttribute('aria-busy', 'false');
  });

  it('keeps the banner, footer and pager inside the busy region', async () => {
    const next = held(hostsAnswer([hostRowFixture()]));
    await open('/hosts', [
      hostsAnswer([hostRowFixture()], { total: 120, metrics_available: false }),
      next.responder,
    ]);
    fireEvent.change(screen.getByRole('combobox', { name: 'State' }), { target: { value: 'ok' } });
    await advance(0);
    const busy = region();
    expect(busy).toHaveAttribute('aria-busy', 'true');
    expect(
      within(busy).getByText('Current values are not available right now.'),
    ).toBeInTheDocument();
    expect(within(busy).getByText('120 hosts')).toBeInTheDocument();
    expect(within(busy).getByRole('navigation', { name: 'Pages' })).toBeInTheDocument();
  });
});

describe('an answer with hosts counted but none listed', () => {
  it('says Nothing on this page. and keeps the pager', async () => {
    await open('/hosts?offset=0', hostsAnswer([], { total: 3 }));
    expect(screen.getByText('Nothing on this page.')).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Pages' })).toBeInTheDocument();
    expect(screen.getByText('3 hosts')).toBeInTheDocument();
  });
});

describe('group in the address that is too long', () => {
  it('sends no group, warns and offers a link that removes it', async () => {
    await open(
      '/hosts?search=a&group=' + 'g'.repeat(64) + '&offset=50',
      hostsAnswer([hostRowFixture()]),
    );
    expect(lastHostQuery().has('group')).toBe(false);
    expect(screen.getByText(/The group in the address is not valid\./)).toBeInTheDocument();
    const link = screen.getByRole('link', { name: 'Show all groups' });
    expect(link).toHaveAttribute('href', '/hosts?search=a');
    fireEvent.click(link);
    await advance(0);
    expect(window.location.search).toBe('?search=a');
    expect(screen.queryByText(/The group in the address is not valid\./)).not.toBeInTheDocument();
  });
});

describe('search field details', () => {
  it('does not write the URL or ask again when the text ends up as it was', async () => {
    await open('/hosts?search=web&offset=50', hostsAnswer([hostRowFixture()], { total: 80 }));
    const writes = vi.spyOn(window.history, 'replaceState');
    const input = screen.getByRole('searchbox', { name: 'Search' });
    fireEvent.input(input, { target: { value: 'webx' } });
    fireEvent.input(input, { target: { value: 'web' } });
    await advance(300);
    fireEvent.submit(input);
    await advance(0);
    expect(writes).not.toHaveBeenCalled();
    expect(window.location.search).toBe('?search=web&offset=50');
    expect(hostRequests()).toHaveLength(1);
    writes.mockRestore();
  });

  it('does nothing on Enter without a change', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    fireEvent.submit(screen.getByRole('searchbox', { name: 'Search' }));
    await advance(0);
    expect(hostRequests()).toHaveLength(1);
  });

  it('returns focus to the search field after Clear search', async () => {
    await open('/hosts?search=web', hostsAnswer([hostRowFixture()]));
    const clear = screen.getByRole('button', { name: 'Clear search' });
    clear.focus();
    fireEvent.click(clear);
    await advance(0);
    expect(screen.getByRole('searchbox', { name: 'Search' })).toHaveFocus();
  });

  it('writes nothing after the page is gone', async () => {
    const view = await open('/hosts', hostsAnswer([hostRowFixture()]));
    fireEvent.input(screen.getByRole('searchbox', { name: 'Search' }), { target: { value: 'db' } });
    view.unmount();
    await advance(500);
    expect(window.location.search).toBe('');
    expect(hostRequests()).toHaveLength(1);
  });

  it('writes the URL once and sends one request when Enter comes during the debounce', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    const writes = vi.spyOn(window.history, 'replaceState');
    const input = screen.getByRole('searchbox', { name: 'Search' });
    fireEvent.input(input, { target: { value: 'db' } });
    fireEvent.submit(input);
    await advance(500);
    expect(writes).toHaveBeenCalledTimes(1);
    expect(hostRequests().filter((r) => r.query.get('search') === 'db')).toHaveLength(1);
    writes.mockRestore();
  });

  it('keeps the group and the typed search when the group changes during the debounce', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    fireEvent.input(screen.getByRole('searchbox', { name: 'Search' }), { target: { value: 'db' } });
    fireEvent.change(screen.getByRole('combobox', { name: 'Group' }), { target: { value: 'web' } });
    await advance(300);
    const url = new URLSearchParams(window.location.search);
    expect(url.get('group')).toBe('web');
    expect(url.get('search')).toBe('db');
    expect(lastHostQuery().get('group')).toBe('web');
    expect(lastHostQuery().get('search')).toBe('db');
    expect(screen.getByRole('searchbox', { name: 'Search' })).toHaveValue('db');
  });
});

describe('other details', () => {
  it('shows what age() says for a last value in the future', async () => {
    const future = new Date(NOW.getTime() + 60_000).toISOString();
    await open('/hosts', hostsAnswer([hostRowFixture({ last_seen_at: future })]));
    expect(rowAt(0)).toHaveTextContent('0 s ago');
  });

  it('loads the groups once, not with every poll', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    await advance(30_000);
    expect(hostRequests().length).toBeGreaterThan(1);
    expect(mock.count('GET /host-groups')).toBe(1);
  });
});

describe('hosts that are not up to date', () => {
  it('says since when the list is old when a later poll fails', async () => {
    await open('/hosts', [hostsAnswer([hostRowFixture()]), 'network']);
    expect(staleNotes()).toHaveLength(0);
    await advance(10_000);
    expectStaleAbove(screen.getByRole('table', { name: 'Hosts' }), '14:30');
  });
});

describe('the group list', () => {
  it('is loaded once while the hosts are polled', async () => {
    await open('/hosts', hostsAnswer([hostRowFixture()]));
    await advance(60_000);
    expect(hostRequests().length).toBeGreaterThan(5);
    expect(mock.count('GET /host-groups')).toBe(1);
  });

  it('is loaded again by Try again after a failed first load', async () => {
    await open('/hosts', [{ status: 500, json: { error: 'internal', message: 'x' } }, hostsAnswer([])], 'network');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(mock.count('GET /host-groups')).toBe(2);
  });
});
