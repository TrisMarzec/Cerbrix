import { render, screen, within } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import type { Role, User } from '../api/types';
import { SessionProvider } from '../session';
import { resolveRoute } from '../shell/routes';
import { NowContext, StatusContext } from '../shell/context';
import { NOW, statusFixture, userFixture } from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { expectStaleAbove, staleNotes } from '../test/stale';
import { AdministrationPage } from './AdministrationPage';

let mock: FetchMock;

const HOSTILE = '<img src=x onerror=alert(1)>';
const GENERATED = 'Xk3-generated-pass';
const ME = userFixture({ id: 'id-anna', name: 'anna', role: 'admin' });
const BOB = userFixture({ id: 'id-bob', name: 'bob', role: 'viewer' });
const CAROL = userFixture({ id: 'id-carol', name: 'carol', role: 'operator', disabled: true });
const DAVE = userFixture({ id: 'id-dave', name: 'dave', role: 'viewer', disabled: true });

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
});

interface Options {
  role?: Role;
  url?: string;
  users?: User[];
  total?: number;
  routes?: Record<string, Responder | Responder[]>;
  /** Render the page through the route table, as the shell does. */
  viaRoute?: boolean;
}

async function open(o: Options = {}) {
  window.history.replaceState(null, '', o.url ?? '/administration');
  const users = o.users ?? [ME, BOB, CAROL];
  mock = mockFetch({
    'GET /session': { json: { name: 'anna', role: o.role ?? 'admin' } },
    'GET /users': { json: { items: users, total: o.total ?? users.length } },
    ...o.routes,
  });
  const page = o.viaRoute === true ? resolveRoute('/administration').element : <AdministrationPage />;
  const view = render(
    <SessionProvider>
      <StatusContext.Provider value={{ status: statusFixture(), staleSince: undefined, reloadStatus: () => {} }}>
        <NowContext.Provider value={NOW}>{page}</NowContext.Provider>
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

function dialog(name: string) {
  return screen.getByRole('dialog', { name });
}

async function openAdd(u: ReturnType<typeof user>) {
  await u.click(screen.getByRole('button', { name: 'Add user' }));
  return dialog('Add user');
}

const NAME_PROBLEM =
  'Use 1 to 63 characters: a–z, 0–9, dot, underscore, hyphen; start with a letter or digit.';

describe('AdministrationPage', () => {
  it('lists the users with the own row marked and disabled users in words', async () => {
    await open();
    expect(screen.getByRole('heading', { level: 1, name: 'Administration' })).toBeTruthy();
    expect(screen.getByRole('heading', { level: 2, name: 'Users' })).toBeTruthy();
    const rows = screen.getAllByRole('row').slice(1);
    expect(rows).toHaveLength(3);
    const own = within(rows[0] as HTMLElement);
    expect(own.getByText('anna')).toBeTruthy();
    expect(own.getByText('you')).toBeTruthy();
    expect(own.getByText('Admin')).toBeTruthy();
    expect(own.getByText('Active')).toBeTruthy();
    const bob = within(rows[1] as HTMLElement);
    expect(bob.queryByText('you')).toBeNull();
    expect(bob.getByText('Viewer')).toBeTruthy();
    const carol = within(rows[2] as HTMLElement);
    expect(carol.getByText('Disabled')).toBeTruthy();
    expect(carol.getByText('Operator')).toBeTruthy();
    expect(carol.getByRole('button', { name: 'Enable carol' })).toBeTruthy();
    expect(bob.getByRole('button', { name: 'Disable bob' })).toBeTruthy();
    const cells = within(rows[0] as HTMLElement).getAllByRole('cell');
    expect(cells.some((c) => c.querySelector('time') !== null)).toBe(true);
    const first = sent('GET', '/users')[0];
    expect(first?.query.get('limit')).toBe('50');
    expect(first?.query.get('offset')).toBe('0');
  });

  it('shows nothing but the heading before the first answer', async () => {
    window.history.replaceState(null, '', '/administration');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'admin' } },
      'GET /users': () => new Promise(() => {}),
    });
    render(
      <SessionProvider>
        <NowContext.Provider value={NOW}>
          <AdministrationPage />
        </NowContext.Provider>
      </SessionProvider>,
    );
    await advance(0);
    await advance(0);
    expect(screen.getByRole('heading', { level: 1, name: 'Administration' })).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
    expect(screen.queryByText('No users.')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows a failure of the first load with Try again', async () => {
    await open({
      routes: {
        'GET /users': [
          { status: 500, json: { error: 'internal', message: 'x' } },
          { json: { items: [BOB], total: 1 } },
        ],
      },
    });
    expect(screen.getByText(/Could not load the users\./)).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
    await user().click(screen.getByRole('button', { name: 'Try again' }));
    await advance(0);
    expect(screen.getByText('bob')).toBeTruthy();
    expect(screen.queryByText(/Could not load/)).toBeNull();
  });

  it('falls back to the first page for a garbage offset', async () => {
    await open({ url: '/administration?offset=-5' });
    expect(sent('GET', '/users')[0]?.query.get('offset')).toBe('0');
    mock.restore();
    await open({ url: '/administration?offset=abc' });
    expect(sent('GET', '/users')[0]?.query.get('offset')).toBe('0');
  });

  it('pages with the offset in the URL and dims the old rows while the next page loads', async () => {
    let release: (() => void) | undefined;
    const second = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      total: 120,
      routes: {
        'GET /users': async (r) => {
          if (r.query.get('offset') === '50') {
            await second;
            return { json: { items: [BOB], total: 120 } };
          }
          return { json: { items: [ME], total: 120 } };
        },
      },
    });
    await user().click(screen.getByRole('button', { name: 'Next' }));
    await advance(0);
    expect(window.location.search).toBe('?offset=50');
    expect(document.querySelector('[aria-busy="true"]')).not.toBeNull();
    expect(screen.getByText('anna')).toBeTruthy();
    expect(screen.getByText('1–50 of 120')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next' })).toHaveAttribute('aria-disabled', 'true');
    release?.();
    await advance(0);
    await advance(0);
    expect(document.querySelector('[aria-busy="true"]')).toBeNull();
    expect(screen.getByText('bob')).toBeTruthy();
    expect(sent('GET', '/users').some((r) => r.query.get('offset') === '50')).toBe(true);
  });

  it('shows a hostile name as text', async () => {
    await open({ users: [ME, userFixture({ id: 'id-x', name: HOSTILE })] });
    expect(screen.getByText(HOSTILE)).toBeTruthy();
    expect(document.querySelector('img')).toBeNull();
  });

  it('creates a user with a generated password, shows it once and reloads the list', async () => {
    await open({
      routes: {
        'POST /users': {
          status: 201,
          json: { ...userFixture({ id: 'id-dora', name: 'dora' }), password: GENERATED },
        },
      },
    });
    const u = user();
    const add = await openAdd(u);
    const inAdd = within(add);
    expect((inAdd.getByLabelText('Role') as HTMLSelectElement).value).toBe('viewer');
    expect((inAdd.getByLabelText('Generate a password') as HTMLInputElement).checked).toBe(true);
    await u.type(inAdd.getByLabelText('Name'), 'dora');
    await u.selectOptions(inAdd.getByLabelText('Role'), 'operator');
    const before = sent('GET', '/users').length;
    await u.click(inAdd.getByRole('button', { name: 'Add user' }));
    await advance(0);
    await advance(0);
    const post = sent('POST', '/users');
    expect(post).toHaveLength(1);
    expect(post[0]?.body).toEqual({ name: 'dora', role: 'operator' });
    expect(sent('GET', '/users').length).toBeGreaterThan(before);
    const secret = dialog('Password of dora');
    expect((within(secret).getByRole('textbox') as HTMLInputElement).value).toBe(GENERATED);
    expect(
      within(secret).getByText(
        'Give this password to the user. They should change it after the first login.',
      ),
    ).toBeTruthy();
    await u.click(within(secret).getByRole('button', { name: 'Close' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.body.textContent).not.toContain(GENERATED);
  });

  it('still shows the generated password when the reload after creating fails', async () => {
    await open({
      routes: {
        'POST /users': {
          status: 201,
          json: { ...userFixture({ name: 'dora' }), password: GENERATED },
        },
      },
    });
    const u = user();
    const add = await openAdd(u);
    await u.type(within(add).getByLabelText('Name'), 'dora');
    mock.on('GET /users', 'network');
    await u.click(within(add).getByRole('button', { name: 'Add user' }));
    await advance(0);
    await advance(0);
    const field = within(dialog('Password of dora')).getByRole('textbox') as HTMLInputElement;
    expect(field.value).toBe(GENERATED);
  });

  it('creates a user with a chosen password, sends it and shows no secret', async () => {
    await open({
      routes: { 'POST /users': { status: 201, json: userFixture({ id: 'id-dora', name: 'dora' }) } },
    });
    const u = user();
    const add = await openAdd(u);
    const inAdd = within(add);
    await u.type(inAdd.getByLabelText('Name'), 'dora');
    await u.click(inAdd.getByLabelText('Set a password'));
    await u.type(inAdd.getByLabelText('Password'), 'correct horse battery');
    await u.type(inAdd.getByLabelText('Repeat the password'), 'correct horse battery');
    await u.click(inAdd.getByRole('button', { name: 'Add user' }));
    await advance(0);
    await advance(0);
    expect(sent('POST', '/users')[0]?.body).toEqual({
      name: 'dora',
      role: 'viewer',
      password: 'correct horse battery',
    });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.body.textContent).not.toContain('correct horse battery');
  });

  it('refuses invalid names without a request', async () => {
    await open();
    const u = user();
    const add = await openAdd(u);
    const inAdd = within(add);
    const submit = () => u.click(inAdd.getByRole('button', { name: 'Add user' }));

    // An empty name.
    await submit();
    expect(inAdd.getByRole('alert').textContent).toBe(NAME_PROBLEM);
    const bad = ['Anna', '-dora', '.dora', 'dora ', 'müller', 'do ra', 'a'.repeat(64)];
    for (const name of bad) {
      await u.clear(inAdd.getByLabelText('Name'));
      await u.type(inAdd.getByLabelText('Name'), name);
      await submit();
      expect(inAdd.getByRole('alert').textContent).toBe(NAME_PROBLEM);
    }
    expect(sent('POST', '/users')).toHaveLength(0);
  });

  it('refuses a short or unequal password without a request', async () => {
    await open();
    const u = user();
    const add = await openAdd(u);
    const inAdd = within(add);
    const submit = () => u.click(inAdd.getByRole('button', { name: 'Add user' }));
    await u.type(inAdd.getByLabelText('Name'), 'dora');
    await u.click(inAdd.getByLabelText('Set a password'));
    await u.type(inAdd.getByLabelText('Password'), 'short');
    await u.type(inAdd.getByLabelText('Repeat the password'), 'short');
    await submit();
    expect(inAdd.getByRole('alert').textContent).toBe('Use 12 to 256 characters for the password.');
    await u.clear(inAdd.getByLabelText('Password'));
    await u.clear(inAdd.getByLabelText('Repeat the password'));
    await u.type(inAdd.getByLabelText('Password'), 'correct horse battery');
    await u.type(inAdd.getByLabelText('Repeat the password'), 'correct horse batterX');
    await submit();
    expect(inAdd.getByRole('alert').textContent).toBe('The two passwords are not equal.');
    expect(sent('POST', '/users')).toHaveLength(0);
  });

  it('keeps the dialog open on 409 for a taken name and empties the password fields', async () => {
    await open({
      routes: { 'POST /users': { status: 409, json: { error: 'conflict', message: 'user exists' } } },
    });
    const u = user();
    const add = await openAdd(u);
    const inAdd = within(add);
    await u.type(inAdd.getByLabelText('Name'), 'bob');
    await u.click(inAdd.getByLabelText('Set a password'));
    await u.type(inAdd.getByLabelText('Password'), 'correct horse battery');
    await u.type(inAdd.getByLabelText('Repeat the password'), 'correct horse battery');
    await u.click(inAdd.getByRole('button', { name: 'Add user' }));
    await advance(0);
    await advance(0);
    expect(inAdd.getByRole('alert').textContent).toBe('This name is taken.');
    expect(screen.getByRole('dialog', { name: 'Add user' })).toBeTruthy();
    expect((inAdd.getByLabelText('Password') as HTMLInputElement).value).toBe('');
    expect((inAdd.getByLabelText('Repeat the password') as HTMLInputElement).value).toBe('');
    expect((inAdd.getByLabelText('Name') as HTMLInputElement).value).toBe('bob');
  });

  it('changes a role with PATCH, only once it differs', async () => {
    await open({ routes: { 'PATCH /users/id-bob': { json: { ...BOB, role: 'operator' } } } });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Change role of bob' }));
    const d = within(dialog('Change role'));
    expect((d.getByLabelText('Role') as HTMLSelectElement).value).toBe('viewer');
    expect((d.getByRole('button', { name: 'Change role' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(d.queryByText('This ends your own session.')).toBeNull();
    await u.selectOptions(d.getByLabelText('Role'), 'operator');
    const before = sent('GET', '/users').length;
    await u.click(d.getByRole('button', { name: 'Change role' }));
    await advance(0);
    await advance(0);
    expect(sent('PATCH', '/users/id-bob')[0]?.body).toEqual({ role: 'operator' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(sent('GET', '/users').length).toBeGreaterThan(before);
  });

  it('shows the server message when the last admin cannot be demoted', async () => {
    await open({
      routes: {
        'PATCH /users/id-anna': {
          status: 409,
          json: { error: 'conflict', message: 'the last enabled admin cannot be changed' },
        },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Change role of anna' }));
    const d = within(dialog('Change role'));
    expect(d.getByText('This ends your own session.')).toBeTruthy();
    await u.selectOptions(d.getByLabelText('Role'), 'viewer');
    await u.click(d.getByRole('button', { name: 'Change role' }));
    await advance(0);
    await advance(0);
    expect(d.getByRole('alert').textContent).toBe('the last enabled admin cannot be changed');
    expect(screen.getByRole('dialog', { name: 'Change role' })).toBeTruthy();
  });

  it('asks before disabling and sends PATCH disabled', async () => {
    await open({ routes: { 'PATCH /users/id-bob': { json: { ...BOB, disabled: true } } } });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Disable bob' }));
    const d = within(dialog('Disable user'));
    expect(
      d.getByText('Disable bob? They are logged out at once and cannot log in until enabled.'),
    ).toBeTruthy();
    expect(d.queryByText('This ends your own session.')).toBeNull();
    expect(sent('PATCH', '/users/id-bob')).toHaveLength(0);
    await u.click(d.getByRole('button', { name: 'Disable user' }));
    await advance(0);
    await advance(0);
    expect(sent('PATCH', '/users/id-bob')[0]?.body).toEqual({ disabled: true });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('enables at once without asking, with every Enable button disabled meanwhile', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      users: [ME, CAROL, DAVE],
      routes: {
        'PATCH /users/id-carol': async () => {
          await gate;
          return { json: { ...CAROL, disabled: false } };
        },
      },
    });
    const u = user();
    const enableCarol = () => screen.getByRole('button', { name: 'Enable carol' });
    const enableDave = () => screen.getByRole('button', { name: 'Enable dave' });
    await u.click(enableCarol());
    await advance(0);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect((enableCarol() as HTMLButtonElement).disabled).toBe(true);
    expect((enableDave() as HTMLButtonElement).disabled).toBe(true);
    release?.();
    await advance(0);
    await advance(0);
    expect(sent('PATCH', '/users/id-carol')).toHaveLength(1);
    expect(sent('PATCH', '/users/id-carol')[0]?.body).toEqual({ disabled: false });
    expect((enableCarol() as HTMLButtonElement).disabled).toBe(false);
    expect((enableDave() as HTMLButtonElement).disabled).toBe(false);
  });

  it('clears the Enable failure after a successful Enable, another action or another page', async () => {
    await open({
      users: [ME, BOB, CAROL],
      total: 120,
      routes: {
        'PATCH /users/id-carol': [
          { status: 429, json: { error: 'too_many_attempts', message: 'busy' } },
          { json: { ...CAROL, disabled: false } },
          { status: 403, json: { error: 'forbidden', message: 'no' } },
          { status: 403, json: { error: 'forbidden', message: 'no' } },
        ],
        'PATCH /users/id-bob': { json: { ...BOB, disabled: true } },
      },
    });
    const u = user();
    const enable = async () => {
      await u.click(screen.getByRole('button', { name: 'Enable carol' }));
      await advance(0);
      await advance(0);
    };
    await enable();
    expect(screen.getByRole('alert').textContent).toBe('The server is busy, try again in a moment.');
    // The same user enabled successfully.
    await enable();
    expect(screen.queryByRole('alert')).toBeNull();
    // A later action that succeeds.
    await enable();
    expect(screen.getByRole('alert').textContent).toBe('Your role may not do this.');
    await u.click(screen.getByRole('button', { name: 'Disable bob' }));
    await u.click(within(dialog('Disable user')).getByRole('button', { name: 'Disable user' }));
    await advance(0);
    await advance(0);
    expect(screen.queryByRole('alert')).toBeNull();
    // Another page.
    await enable();
    expect(screen.getByRole('alert')).toBeTruthy();
    await u.click(screen.getByRole('button', { name: 'Next' }));
    await advance(0);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('warns about the own session when changing, disabling or removing oneself only', async () => {
    await open();
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Disable anna' }));
    expect(within(dialog('Disable user')).getByText(/This ends your own session\./)).toBeTruthy();
    await u.click(within(dialog('Disable user')).getByRole('button', { name: 'Cancel' }));
    await u.click(screen.getByRole('button', { name: 'Remove anna' }));
    expect(within(dialog('Remove user')).getByText(/This ends your own session\./)).toBeTruthy();
    await u.click(within(dialog('Remove user')).getByRole('button', { name: 'Cancel' }));
    await u.click(screen.getByRole('button', { name: 'Remove bob' }));
    expect(within(dialog('Remove user')).queryByText(/This ends your own session\./)).toBeNull();
    expect(sent('PATCH', '/users/id-anna')).toHaveLength(0);
  });

  it('offers no password reset for the own row, only a hint, and does for the others', async () => {
    await open();
    const rows = screen.getAllByRole('row').slice(1);
    const own = within(rows[0] as HTMLElement);
    expect(own.queryByRole('button', { name: /Reset password/ })).toBeNull();
    expect(own.getByText('Use Change password for your own account.')).toBeTruthy();
    for (const [i, name] of [[1, 'bob'], [2, 'carol']] as const) {
      const row = within(rows[i] as HTMLElement);
      expect(row.getByRole('button', { name: `Reset password of ${name}` })).toBeTruthy();
      expect(row.queryByText('Use Change password for your own account.')).toBeNull();
    }
  });

  it('resets a password to a generated one shown once', async () => {
    await open({ routes: { 'POST /users/id-bob/password': { json: { password: GENERATED } } } });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Reset password of bob' }));
    const d = within(dialog('Reset password'));
    expect(d.getByText('All sessions of bob end.')).toBeTruthy();
    expect((d.getByLabelText('Generate a password') as HTMLInputElement).checked).toBe(true);
    await u.click(d.getByRole('button', { name: 'Reset password' }));
    await advance(0);
    await advance(0);
    expect(sent('POST', '/users/id-bob/password')[0]?.body).toEqual({});
    const secret = dialog('Password of bob');
    expect((within(secret).getByRole('textbox') as HTMLInputElement).value).toBe(GENERATED);
    await u.click(within(secret).getByRole('button', { name: 'Close' }));
    expect(document.body.textContent).not.toContain(GENERATED);
  });

  it('resets a password to a chosen one without showing a secret', async () => {
    await open({ routes: { 'POST /users/id-bob/password': { json: {} } } });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Reset password of bob' }));
    const d = within(dialog('Reset password'));
    await u.click(d.getByLabelText('Set a password'));
    await u.type(d.getByLabelText('Password'), 'correct horse battery');
    await u.type(d.getByLabelText('Repeat the password'), 'correct horse battery');
    await u.click(d.getByRole('button', { name: 'Reset password' }));
    await advance(0);
    await advance(0);
    expect(sent('POST', '/users/id-bob/password')[0]?.body).toEqual({
      password: 'correct horse battery',
    });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('asks naming the user before removing and sends DELETE', async () => {
    await open({ routes: { 'DELETE /users/id-bob': { status: 204 } } });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Remove bob' }));
    const d = within(dialog('Remove user'));
    expect(d.getByText('Remove bob? This cannot be undone.')).toBeTruthy();
    expect(sent('DELETE', '/users/id-bob')).toHaveLength(0);
    await u.click(d.getByRole('button', { name: 'Remove user' }));
    await advance(0);
    await advance(0);
    expect(sent('DELETE', '/users/id-bob')).toHaveLength(1);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('says so on 429, 403 and a network failure and stays open', async () => {
    await open({
      routes: {
        'POST /users/id-bob/password': [
          { status: 429, json: { error: 'too_many_attempts', message: 'busy' } },
          { status: 403, json: { error: 'forbidden', message: 'no' } },
          'network',
        ],
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Reset password of bob' }));
    const d = within(dialog('Reset password'));
    const submit = async () => {
      await u.click(d.getByRole('button', { name: 'Reset password' }));
      await advance(0);
      await advance(0);
    };
    await submit();
    expect(d.getByRole('alert').textContent).toBe('The server is busy, try again in a moment.');
    await submit();
    expect(d.getByRole('alert').textContent).toBe('Your role may not do this.');
    await submit();
    expect(d.getByRole('alert').textContent).toBe('Could not reach the server.');
    expect(screen.getByRole('dialog', { name: 'Reset password' })).toBeTruthy();
  });

  it('says the user is gone on 404 and reloads when the dialog closes', async () => {
    await open({
      routes: { 'DELETE /users/id-bob': { status: 404, json: { error: 'not_found', message: 'x' } } },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Remove bob' }));
    const d = within(dialog('Remove user'));
    await u.click(d.getByRole('button', { name: 'Remove user' }));
    await advance(0);
    await advance(0);
    expect(d.getByRole('alert').textContent).toBe('This user no longer exists.');
    const before = sent('GET', '/users').length;
    await u.click(d.getByRole('button', { name: 'Cancel' }));
    await advance(0);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(sent('GET', '/users').length).toBeGreaterThan(before);
  });

  it('cannot be dismissed while the request runs, and runs once', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await open({
      routes: {
        'DELETE /users/id-bob': async () => {
          await gate;
          return { status: 204 };
        },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Remove bob' }));
    const d = within(dialog('Remove user'));
    await u.click(d.getByRole('button', { name: 'Remove user' }));
    await advance(0);
    expect((d.getByRole('button', { name: 'Remove user' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect((d.getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(true);
    await u.keyboard('{Escape}');
    expect(screen.getByRole('dialog', { name: 'Remove user' })).toBeTruthy();
    release?.();
    await advance(0);
    await advance(0);
    expect(sent('DELETE', '/users/id-bob')).toHaveLength(1);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('does not warn or throw when the page unmounts while a request is in flight', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    const warnings = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const view = await open({
      routes: {
        'POST /users/id-bob/password': async () => {
          await gate;
          return { json: { password: GENERATED } };
        },
      },
    });
    const u = user();
    await u.click(screen.getByRole('button', { name: 'Reset password of bob' }));
    await u.click(
      within(dialog('Reset password')).getByRole('button', { name: 'Reset password' }),
    );
    await advance(0);
    view.unmount();
    release?.();
    await advance(0);
    await advance(0);
    expect(errors).not.toHaveBeenCalled();
    expect(warnings).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain(GENERATED);
    errors.mockRestore();
    warnings.mockRestore();
  });

  it('refuses a non-admin and sends no request for users', async () => {
    await open({ role: 'operator', viaRoute: true });
    expect(screen.getByText('Not allowed for your role')).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
    expect(mock.requests.filter((r) => r.path !== '/session')).toHaveLength(0);
  });
});

describe('users that are not up to date', () => {
  it('says since when the list is old when a later poll fails', async () => {
    await open({ routes: { 'GET /users': [{ json: { items: [ME, BOB], total: 2 } }, 'network'] } });
    expect(staleNotes()).toHaveLength(0);
    await advance(10_000);
    expect(screen.getByRole('table', { name: 'Users' }).parentElement).toHaveClass('table-scroll');
    expectStaleAbove(screen.getByRole('table', { name: 'Users' }), '14:30');
  });
});

describe('password managers', () => {
  it('marks the new user name as no login name and the passwords as new ones', async () => {
    await open();
    const u = user();
    const inAdd = within(await openAdd(u));
    expect(inAdd.getByLabelText('Name')).toHaveAttribute('autocomplete', 'off');
    await u.click(inAdd.getByLabelText('Set a password'));
    expect(inAdd.getByLabelText('Password')).toHaveAttribute('autocomplete', 'new-password');
    expect(inAdd.getByLabelText('Repeat the password')).toHaveAttribute('autocomplete', 'new-password');
  });
});
