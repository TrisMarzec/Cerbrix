import { render, screen, within } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import type { Role, Status, SyncChanges } from '../api/types';
import { SessionProvider } from '../session';
import { NowContext, StatusContext } from '../shell/context';
import {
  displayFixture,
  keyFixture,
  NOW,
  statusFixture,
  syncEntryFixture,
} from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { expectStaleAbove, staleNotes } from '../test/stale';
import { ConfigurationPage } from './ConfigurationPage';

let mock: FetchMock;

const HOSTILE = '<img src=x onerror=alert(1)>';
const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
  if (original === undefined) Reflect.deleteProperty(navigator, 'clipboard');
  else Object.defineProperty(navigator, 'clipboard', original);
});

interface Options {
  role?: Role;
  status?: Status | undefined;
  url?: string;
  routes?: Record<string, Responder | Responder[]>;
}

async function open(o: Options = {}) {
  window.history.replaceState(null, '', o.url ?? '/configuration');
  mock = mockFetch({
    'GET /session': { json: { name: 'anna', role: o.role ?? 'viewer' } },
    'GET /sync/history': { json: { items: [], total: 0 } },
    'GET /unknown-agents': { json: { items: [] } },
    'GET /enrollment-keys': { json: { items: [] } },
    'GET /displays': { json: { items: [] } },
    ...o.routes,
  });
  const status = 'status' in o ? o.status : statusFixture();
  const view = render(
    <SessionProvider>
      <StatusContext.Provider value={{ status, staleSince: undefined }}>
        <NowContext.Provider value={NOW}>
          <ConfigurationPage />
        </NowContext.Provider>
      </StatusContext.Provider>
    </SessionProvider>,
  );
  await advance(0);
  await advance(0);
  await advance(0);
  return view;
}

function user() {
  return userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
}

function sent(method: string, path: string) {
  return mock.requests.filter((r) => r.method === method && r.path === path);
}

function paths() {
  return mock.requests.map((r) => r.path).filter((p) => p !== '/session');
}

function changes(overrides: Partial<SyncChanges>): SyncChanges {
  return { ...syncEntryFixture().changes, ...overrides };
}

function names(...changed: string[]) {
  return { added: [], changed, removed: [] };
}

function dialog(): HTMLElement {
  return screen.getByRole('dialog');
}

describe('page', () => {
  it('shows only the heading of the synchronisation before the status is there', async () => {
    await open({ status: undefined });
    expect(screen.getByRole('heading', { level: 1, name: 'Configuration' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 2, name: 'Synchronisation' })).toBeInTheDocument();
    expect(screen.queryByText('State')).not.toBeInTheDocument();
    expect(sent('GET', '/sync/history')).toHaveLength(0);
  });

  it('lists the sections of an admin in order', async () => {
    await open({ role: 'admin' });
    expect(screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent)).toEqual([
      'Synchronisation',
      'Sync history',
      'Unknown agents',
      'Enrolment keys',
      'Display tokens',
    ]);
  });

  it('shows a viewer the first two sections and sends no request for the others', async () => {
    await open({ role: 'viewer' });
    expect(screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent)).toEqual([
      'Synchronisation',
      'Sync history',
    ]);
    expect(paths()).toEqual(['/sync/history']);
  });

  it('shows an operator the unknown agents but not keys or tokens', async () => {
    await open({ role: 'operator' });
    expect(screen.getByRole('heading', { level: 2, name: 'Unknown agents' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Enrolment keys' })).not.toBeInTheDocument();
    expect(paths().sort()).toEqual(['/sync/history', '/unknown-agents']);
  });

  it('keeps the other sections when one fails, and retries only that one', async () => {
    await open({
      role: 'admin',
      routes: {
        'GET /unknown-agents': { status: 500, json: { error: 'internal', message: 'boom' } },
        'GET /displays': { json: { items: [displayFixture()] } },
      },
    });
    expect(screen.getByText('Could not load the unknown agents.')).toBeInTheDocument();
    expect(screen.getByText('No enrolment keys.')).toBeInTheDocument();
    expect(screen.getByText('E2E wall')).toBeInTheDocument();
    mock.on('GET /unknown-agents', { json: { items: [] } });
    await user().click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByText('No unknown agents.')).toBeInTheDocument();
    expect(screen.queryByText('Could not load the unknown agents.')).not.toBeInTheDocument();
  });
});

describe('synchronisation', () => {
  it('says that GitOps is off and shows nothing else', async () => {
    await open({ status: statusFixture({ gitops: false }), role: 'operator' });
    expect(
      screen.getByText((_, el) => el?.tagName === 'P' && el.textContent === 'GitOps is off. Hosts are managed with cerbrix-server admin.'),
    ).toBeInTheDocument();
    expect(screen.queryByText('Active commit')).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Sync history' })).not.toBeInTheDocument();
    expect(sent('GET', '/sync/history')).toHaveLength(0);
  });

  it('shows the state, the full commit and both times', async () => {
    await open();
    expect(screen.getByText('In sync')).toBeInTheDocument();
    expect(screen.getByText('253a2994b13aca387e272ff08db9eb858b436b66')).toBeInTheDocument();
    expect(screen.getByText('Last successful synchronisation')).toBeInTheDocument();
    expect(screen.getByText('14:28')).toBeInTheDocument();
    expect(screen.getByText('14:29')).toBeInTheDocument();
  });

  it('shows "never" and "none" before the first synchronisation', async () => {
    await open({
      status: statusFixture({
        sync: {
          status: 'never',
          active_commit: null,
          last_success_at: null,
          last_attempt_at: null,
          issues: [],
        },
      }),
    });
    expect(screen.getByText('Not synchronised yet')).toBeInTheDocument();
    expect(screen.getByText('none')).toBeInTheDocument();
    expect(screen.getAllByText('never')).toHaveLength(2);
    expect(screen.queryByRole('button', { name: 'Copy' })).not.toBeInTheDocument();
  });

  it('shows an out-of-sync state with its issues, positions and a hostile reason as text', async () => {
    await open({
      status: statusFixture({
        sync: {
          ...statusFixture().sync,
          status: 'out_of_sync',
          issues: [
            { file: 'hosts/a.json', line: 3, column: 14, reason: 'unknown key <b>x</b>' },
            { file: 'hosts/b.json', line: 7, column: null, reason: 'bad value' },
            { file: null, line: null, column: null, reason: 'repository unreachable' },
          ],
        },
      }),
    });
    expect(screen.getByText('Out of sync')).toBeInTheDocument();
    const rows = within(screen.getByRole('table', { name: 'Synchronisation issues' })).getAllByRole('row');
    expect(rows).toHaveLength(4);
    expect(within(rows[1] as HTMLElement).getAllByRole('cell').map((c) => c.textContent)).toEqual([
      'hosts/a.json',
      '3:14',
      'unknown key <b>x</b>',
    ]);
    expect(within(rows[2] as HTMLElement).getAllByRole('cell')[1]).toHaveTextContent(/^7$/);
    expect(within(rows[3] as HTMLElement).getAllByRole('cell').map((c) => c.textContent)).toEqual([
      '–',
      '–',
      'repository unreachable',
    ]);
    expect(document.querySelector('td b')).toBeNull();
  });

  it('copies the commit and says "Copied" for two seconds', async () => {
    // user-event installs its own clipboard when it is set up, so set up first.
    const u = user();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    await open();
    await u.click(screen.getByRole('button', { name: 'Copy' }));
    await advance(0);
    expect(writeText).toHaveBeenCalledWith('253a2994b13aca387e272ff08db9eb858b436b66');
    expect(screen.getByRole('button', { name: 'Copied' })).toBeInTheDocument();
    await advance(1900);
    expect(screen.getByRole('button', { name: 'Copied' })).toBeInTheDocument();
    await advance(200);
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
  });

  it('selects the hash when there is no clipboard', async () => {
    const u = user();
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
    await open();
    await u.click(screen.getByRole('button', { name: 'Copy' }));
    await advance(0);
    expect(window.getSelection()?.toString()).toBe('253a2994b13aca387e272ff08db9eb858b436b66');
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
    expect(screen.getByText('Press Ctrl+C to copy')).toBeInTheDocument();
  });

  it('selects the hash and says so when the clipboard refuses', async () => {
    const u = user();
    const writeText = vi.fn(() => Promise.reject(new Error('denied')));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    await open();
    await u.click(screen.getByRole('button', { name: 'Copy' }));
    await advance(0);
    expect(window.getSelection()?.toString()).toBe('253a2994b13aca387e272ff08db9eb858b436b66');
    expect(screen.getByText('Press Ctrl+C to copy')).toBeInTheDocument();
  });
});

describe('sync history', () => {
  it('asks for 20 entries from the offset in the URL', async () => {
    await open({
      url: '/configuration?history_offset=40',
      routes: { 'GET /sync/history': { json: { items: [syncEntryFixture()], total: 60 } } },
    });
    const q = sent('GET', '/sync/history')[0]?.query;
    expect(q?.get('limit')).toBe('20');
    expect(q?.get('offset')).toBe('40');
  });

  it.each(['-5', 'abc'])('falls back to the first page on history_offset=%s', async (garbage) => {
    await open({ url: `/configuration?history_offset=${garbage}` });
    expect(sent('GET', '/sync/history')[0]?.query.get('offset')).toBe('0');
  });

  it('moves to the last existing page when the offset is beyond the total', async () => {
    const last = syncEntryFixture({ commit: 'ccccccc0000000000000000000000000000000000' });
    await open({
      url: '/configuration?history_offset=100',
      routes: {
        'GET /sync/history': (r) =>
          r.query.get('offset') === '40'
            ? { json: { items: [last], total: 45 } }
            : { json: { items: [], total: 45 } },
      },
    });
    expect(window.location.search).toBe('?history_offset=40');
    expect(screen.getByText('ccccccc')).toBeInTheDocument();
    expect(sent('GET', '/sync/history').map((r) => r.query.get('offset'))).toEqual(['100', '40']);
  });

  it('shows the empty sentence for an offset beyond an empty history', async () => {
    await open({ url: '/configuration?history_offset=100' });
    expect(screen.getByText('No revision has been applied yet.')).toBeInTheDocument();
    expect(sent('GET', '/sync/history')).toHaveLength(1);
  });

  it('shows the commit short with the full hash as title', async () => {
    await open({ routes: { 'GET /sync/history': { json: { items: [syncEntryFixture()], total: 1 } } } });
    const short = screen.getByText('253a299');
    expect(short).toHaveAttribute('title', '253a2994b13aca387e272ff08db9eb858b436b66');
  });

  it('writes the changes in words, singular and plural, in a fixed order', async () => {
    const entry = syncEntryFixture({
      changes: changes({
        dashboards: { added: ['wall'], changed: [], removed: [] },
        hosts: { added: [], changed: ['a', 'b'], removed: ['old'] },
        displays: { added: [], changed: [], removed: ['x', 'y', 'z'] },
      }),
    });
    await open({ routes: { 'GET /sync/history': { json: { items: [entry], total: 1 } } } });
    expect(
      screen.getByText('2 hosts changed, 1 host removed, 1 dashboard added, 3 displays removed'),
    ).toBeInTheDocument();
  });

  it('says "No changes" when every list is empty and offers no names', async () => {
    await open({ routes: { 'GET /sync/history': { json: { items: [syncEntryFixture()], total: 1 } } } });
    expect(screen.getByText('No changes')).toBeInTheDocument();
    expect(document.querySelector('details')).toBeNull();
  });

  it('lists the names per group below, as text', async () => {
    const entry = syncEntryFixture({
      changes: changes({ hosts: names('a', HOSTILE), templates: { added: ['t'], changed: [], removed: [] } }),
    });
    await open({ routes: { 'GET /sync/history': { json: { items: [entry], total: 1 } } } });
    const details = document.querySelector('details');
    expect(details).not.toBeNull();
    await user().click(screen.getByText('Names'));
    expect(details?.open).toBe(true);
    expect(within(details as HTMLElement).getByText(`Hosts changed: a, ${HOSTILE}`)).toBeInTheDocument();
    expect(within(details as HTMLElement).getByText('Templates added: t')).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
  });

  it('says so when no revision has been applied', async () => {
    await open();
    expect(screen.getByText('No revision has been applied yet.')).toBeInTheDocument();
  });

  it('shows a banner with "Try again" when the first load fails', async () => {
    await open({
      routes: { 'GET /sync/history': [{ status: 500, json: { error: 'internal', message: 'x' } }, { json: { items: [], total: 0 } }] },
    });
    expect(screen.getByText('Could not load the sync history.')).toBeInTheDocument();
    await user().click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByText('No revision has been applied yet.')).toBeInTheDocument();
  });

  it('pages through the URL and dims the old rows meanwhile', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = { json: { items: [syncEntryFixture()], total: 45 } };
    const second: Responder = async () => {
      await gate;
      return { json: { items: [syncEntryFixture({ commit: 'bbbbbbb0000000000000000000000000000000000' })], total: 45 } };
    };
    await open({ routes: { 'GET /sync/history': [first, second] } });
    await user().click(screen.getByRole('button', { name: 'Next' }));
    await advance(0);
    expect(window.location.search).toBe('?history_offset=20');
    const region = document.querySelector('.config-result');
    expect(region).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByText('253a299')).toBeInTheDocument();
    release();
    await advance(0);
    expect(document.querySelector('.config-result')).toHaveAttribute('aria-busy', 'false');
    expect(screen.getByText('bbbbbbb')).toBeInTheDocument();
    expect(sent('GET', '/sync/history').at(-1)?.query.get('offset')).toBe('20');
  });
});

describe('unknown agents', () => {
  it('lists the agents with the note above', async () => {
    const answer = {
      items: [
        {
          host_name: HOSTILE,
          first_seen: new Date(2026, 10, 17, 9, 0).toISOString(),
          last_seen: new Date(2026, 10, 17, 14, 0).toISOString(),
          attempts: 12,
        },
      ],
    };
    await open({ role: 'operator', routes: { 'GET /unknown-agents': { json: answer } } });
    expect(
      screen.getByText('Agents that tried to enrol under a name that is not a registered host.'),
    ).toBeInTheDocument();
    const cells = within(screen.getByRole('table', { name: 'Unknown agents' })).getAllByRole('cell');
    expect(cells.map((c) => c.textContent)).toEqual([HOSTILE, '09:00', '14:00', '12']);
    expect(document.querySelector('img')).toBeNull();
  });

  it('says so when there are none', async () => {
    await open({ role: 'operator' });
    expect(screen.getByText('No unknown agents.')).toBeInTheDocument();
  });

  it('polls every 10 seconds', async () => {
    await open({ role: 'operator' });
    expect(sent('GET', '/unknown-agents')).toHaveLength(1);
    await advance(10_000);
    expect(sent('GET', '/unknown-agents')).toHaveLength(2);
  });
});

describe('enrolment keys', () => {
  const list = (...items: ReturnType<typeof keyFixture>[]) => ({
    'GET /enrollment-keys': { json: { items } },
  });

  it('shows the state as a word and offers Revoke for active keys only', async () => {
    await open({
      role: 'admin',
      routes: list(
        keyFixture({ id: 'a', description: HOSTILE }),
        keyFixture({ id: 'b', description: 'old', state: 'expired' }),
        keyFixture({ id: 'c', description: 'gone', state: 'revoked', revoked_at: NOW.toISOString() }),
      ),
    });
    const rows = within(screen.getByRole('table', { name: 'Enrolment keys' })).getAllByRole('row');
    expect(within(rows[1] as HTMLElement).getByText(HOSTILE)).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText('Active')).toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).getByText('Expired')).toBeInTheDocument();
    expect(within(rows[3] as HTMLElement).getByText('Revoked')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /^Revoke key/ })).toHaveLength(1);
    expect(document.querySelector('img')).toBeNull();
  });

  it('creates a key, shows it once, and the key is gone from the page after closing', async () => {
    await open({
      role: 'admin',
      routes: {
        'POST /enrollment-keys': { status: 201, json: { ...keyFixture(), key: 'cbx_ek_SECRETVALUE' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Create key' }));
    expect(within(dialog()).getByRole('combobox', { name: 'Validity' })).toHaveValue('24h');
    await u.type(within(dialog()).getByRole('textbox', { name: 'Description' }), 'web-02');
    await u.selectOptions(within(dialog()).getByRole('combobox', { name: 'Validity' }), '7d');
    mock.on('GET /enrollment-keys', { json: { items: [keyFixture()] } });
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    await advance(0);

    const post = sent('POST', '/enrollment-keys');
    expect(post).toHaveLength(1);
    expect(post[0]?.body).toEqual({ description: 'web-02', expires_in: '7d' });
    expect(screen.getByDisplayValue('cbx_ek_SECRETVALUE')).toBeInTheDocument();
    expect(screen.getByText("Give this key to the agent's installer.")).toBeInTheDocument();
    expect(sent('GET', '/enrollment-keys')).toHaveLength(2);
    expect(screen.getByRole('table', { name: 'Enrolment keys' })).toBeInTheDocument();

    await u.click(screen.getByRole('button', { name: 'Close' }));
    await advance(0);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue('cbx_ek_SECRETVALUE')).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain('SECRETVALUE');
  });

  it.each([
    ['', 'Enter a description.'],
    ['   ', 'Enter a description.'],
    ['a'.repeat(201), 'The description may have at most 200 characters.'],
    ['bad\u0007name', 'The description must not contain control characters.'],
  ])('refuses the description %j without a request', async (text, message) => {
    await open({ role: 'admin' });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Create key' }));
    const field = within(dialog()).getByRole('textbox', { name: 'Description' });
    if (text !== '') await u.click(field);
    if (text !== '') await u.paste(text);
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    expect(within(dialog()).getByRole('alert')).toHaveTextContent(message);
    expect(sent('POST', '/enrollment-keys')).toHaveLength(0);
  });

  it('accepts exactly 200 characters', async () => {
    await open({
      role: 'admin',
      routes: { 'POST /enrollment-keys': { status: 201, json: { ...keyFixture(), key: 'k' } } },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Create key' }));
    await u.click(within(dialog()).getByRole('textbox', { name: 'Description' }));
    await u.paste('a'.repeat(200));
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    await advance(0);
    expect(sent('POST', '/enrollment-keys')).toHaveLength(1);
  });

  it('keeps the dialog open and says why when the server refuses', async () => {
    await open({
      role: 'admin',
      routes: { 'POST /enrollment-keys': { status: 403, json: { error: 'forbidden', message: 'no' } } },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Create key' }));
    await u.type(within(dialog()).getByRole('textbox', { name: 'Description' }), 'x');
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent('Your role may not do this.');
    mock.on('POST /enrollment-keys', 'network');
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent('Could not reach the server.');
    mock.on('POST /enrollment-keys', { status: 400, json: { error: 'invalid', message: 'description is wrong' } });
    await u.click(within(dialog()).getByRole('button', { name: 'Create key' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent('description is wrong');
  });

  it('asks before revoking, then deletes the key and reloads', async () => {
    await open({
      role: 'admin',
      routes: {
        ...list(keyFixture({ id: 'k 1', description: 'web-02' })),
        'DELETE /enrollment-keys/k%201': { status: 204 },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Revoke key web-02' }));
    expect(sent('DELETE', '/enrollment-keys/k%201')).toHaveLength(0);
    expect(
      within(dialog()).getByText(
        'Revoke the key "web-02"? Agents that have not enrolled yet can no longer use it.',
      ),
    ).toBeInTheDocument();
    await u.click(within(dialog()).getByRole('button', { name: 'Cancel' }));
    expect(sent('DELETE', '/enrollment-keys/k%201')).toHaveLength(0);

    await u.click(screen.getByRole('button', { name: 'Revoke key web-02' }));
    await u.click(within(dialog()).getByRole('button', { name: 'Revoke key' }));
    await advance(0);
    expect(sent('DELETE', '/enrollment-keys/k%201')).toHaveLength(1);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(sent('GET', '/enrollment-keys')).toHaveLength(2);
  });

  it('shows a refused revoke inside the dialog', async () => {
    await open({
      role: 'admin',
      routes: {
        ...list(keyFixture()),
        [`DELETE /enrollment-keys/${keyFixture().id}`]: { status: 403, json: { error: 'forbidden', message: 'no' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Revoke key web-02' }));
    await u.click(within(dialog()).getByRole('button', { name: 'Revoke key' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent('Your role may not do this.');
  });

  it('shows a banner with "Try again" when the list cannot be loaded', async () => {
    await open({
      role: 'admin',
      routes: { 'GET /enrollment-keys': { status: 500, json: { error: 'internal', message: 'x' } } },
    });
    expect(screen.getByText('Could not load the enrolment keys.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Create key' })).not.toBeInTheDocument();
  });

  it('offers "Create key" only once the list has answered', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      role: 'admin',
      routes: {
        'GET /enrollment-keys': async () => {
          await gate;
          return { json: { items: [] } };
        },
      },
    });
    expect(screen.queryByRole('button', { name: 'Create key' })).not.toBeInTheDocument();
    release();
    await advance(0);
    expect(screen.getByRole('button', { name: 'Create key' })).toBeInTheDocument();
  });
});

describe('display tokens', () => {
  const withDisplays = (...items: ReturnType<typeof displayFixture>[]) => ({
    'GET /displays': { json: { items } },
  });
  const created = new Date(2026, 10, 17, 8, 0).toISOString();

  it('says so when there are no displays', async () => {
    await open({ role: 'admin' });
    expect(screen.getByText('No wall displays configured.')).toBeInTheDocument();
  });

  it('shows name, id and token state, with a hostile name as text', async () => {
    await open({
      role: 'admin',
      routes: withDisplays(
        displayFixture({ id: 'a', name: HOSTILE }),
        displayFixture({ id: 'b', name: 'Hall', token_created_at: created }),
      ),
    });
    const rows = within(screen.getByRole('table', { name: 'Display tokens' })).getAllByRole('row');
    expect(rows[1]).toHaveTextContent(HOSTILE);
    expect(rows[1]).toHaveTextContent('No token');
    expect(within(rows[1] as HTMLElement).getByRole('button', { name: /Create token/ })).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).queryByRole('button', { name: /Revoke/ })).not.toBeInTheDocument();
    expect(rows[2]).toHaveTextContent('Hall');
    expect(rows[2]).toHaveTextContent('Created 08:00');
    expect(within(rows[2] as HTMLElement).getByRole('button', { name: /Replace token/ })).toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).getByRole('button', { name: /Revoke/ })).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
  });

  it('creates a first token at once and shows it once', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture()),
        'POST /displays/e2e-wall/token': { status: 201, json: { token: 'cbx_dt_TOKENVALUE' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: /Create token/ }));
    await advance(0);
    expect(sent('POST', '/displays/e2e-wall/token')).toHaveLength(1);
    expect(screen.getByDisplayValue('cbx_dt_TOKENVALUE')).toBeInTheDocument();
    expect(
      screen.getByText('Open /display/e2e-wall on the wall monitor and enter this token.'),
    ).toBeInTheDocument();
    expect(sent('GET', '/displays')).toHaveLength(2);
    await u.click(screen.getByRole('button', { name: 'Close' }));
    await advance(0);
    expect(screen.queryByDisplayValue('cbx_dt_TOKENVALUE')).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain('TOKENVALUE');
  });

  it('warns before replacing a token, and then shows the new one', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture({ token_created_at: created })),
        'POST /displays/e2e-wall/token': { status: 201, json: { token: 'cbx_dt_NEWVALUE' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: /Replace token/ }));
    expect(sent('POST', '/displays/e2e-wall/token')).toHaveLength(0);
    expect(within(dialog()).getByText(/The current token stops working at once\./)).toBeInTheDocument();
    await u.click(within(dialog()).getByRole('button', { name: 'Replace token' }));
    await advance(0);
    expect(sent('POST', '/displays/e2e-wall/token')).toHaveLength(1);
    expect(screen.getByDisplayValue('cbx_dt_NEWVALUE')).toBeInTheDocument();
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
  });

  it('explains a 404 inside the confirmation and keeps it open', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture({ token_created_at: created })),
        'POST /displays/e2e-wall/token': { status: 404, json: { error: 'not_found', message: 'x' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: /Replace token/ }));
    await u.click(within(dialog()).getByRole('button', { name: 'Replace token' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent(
      'The configuration is being synchronised. Try again.',
    );
  });

  it('explains a failed first token in the row of that display only', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture(), displayFixture({ id: 'hall', name: 'Hall' })),
        'POST /displays/hall/token': { status: 404, json: { error: 'not_found', message: 'x' } },
      },
    });
    const table = () => within(screen.getByRole('table', { name: 'Display tokens' }));
    await user().click(within(table().getAllByRole('row')[2] as HTMLElement).getByRole('button', { name: /Create token/ }));
    await advance(0);
    const rows = table().getAllByRole('row');
    expect(within(rows[2] as HTMLElement).getByRole('alert')).toHaveTextContent(
      'The configuration is being synchronised. Try again.',
    );
    expect(within(rows[1] as HTMLElement).queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create key' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Create token of Hall' })).toBeEnabled();
  });

  it('disables every other secret-producing control while a secret is made or shown', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture(), displayFixture({ id: 'hall', name: 'Hall', token_created_at: created })),
        'POST /displays/e2e-wall/token': async () => {
          await gate;
          return { status: 201, json: { token: 'cbx_dt_HELD' } };
        },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Create token of E2E wall' }));
    await advance(0);
    expect(screen.getByRole('button', { name: 'Create key' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Replace token of Hall' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Create token of E2E wall' })).toBeDisabled();

    release();
    await advance(0);
    expect(screen.getByDisplayValue('cbx_dt_HELD')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create key' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Replace token of Hall' })).toBeDisabled();

    await u.click(screen.getByRole('button', { name: 'Close' }));
    await advance(0);
    expect(screen.queryByDisplayValue('cbx_dt_HELD')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create key' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Replace token of Hall' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Create token of E2E wall' })).toBeEnabled();
  });

  it('asks before revoking a token, then deletes it and reloads', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture({ token_created_at: created })),
        'DELETE /displays/e2e-wall/token': { status: 204 },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: /Revoke token/ }));
    expect(sent('DELETE', '/displays/e2e-wall/token')).toHaveLength(0);
    expect(within(dialog()).getByText(/Revoke the token of "E2E wall"\?/)).toBeInTheDocument();
    await u.click(within(dialog()).getByRole('button', { name: 'Revoke token' }));
    await advance(0);
    expect(sent('DELETE', '/displays/e2e-wall/token')).toHaveLength(1);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(sent('GET', '/displays')).toHaveLength(2);
  });

  it('shows a refused revoke inside the dialog', async () => {
    await open({
      role: 'admin',
      routes: {
        ...withDisplays(displayFixture({ token_created_at: created })),
        'DELETE /displays/e2e-wall/token': { status: 403, json: { error: 'forbidden', message: 'no' } },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: /Revoke token/ }));
    await u.click(within(dialog()).getByRole('button', { name: 'Revoke token' }));
    await advance(0);
    expect(within(dialog()).getByRole('alert')).toHaveTextContent('Your role may not do this.');
  });
});

describe('sections that are not up to date', () => {
  it('says since when each section is old when its later poll fails', async () => {
    await open({
      role: 'admin',
      routes: {
        'GET /sync/history': [{ json: { items: [syncEntryFixture()], total: 1 } }, 'network'],
        'GET /unknown-agents': [
          { json: { items: [{ host_name: 'stray', first_seen: NOW.toISOString(), last_seen: NOW.toISOString(), attempts: 2 }] } },
          'network',
        ],
        'GET /enrollment-keys': [{ json: { items: [keyFixture()] } }, 'network'],
        'GET /displays': [{ json: { items: [displayFixture()] } }, 'network'],
      },
    });
    expect(staleNotes()).toHaveLength(0);
    await advance(10_000);
    expect(staleNotes()).toHaveLength(4);
    for (const name of ['Sync history', 'Unknown agents', 'Enrolment keys', 'Display tokens']) {
      expectStaleAbove(screen.getByRole('table', { name }), '14:30');
    }
  });
});
