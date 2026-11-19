import { render, screen, within } from '@testing-library/preact';

import { resetServerSkew } from '../api/client';
import type { Role, Status } from '../api/types';
import { App } from '../app';
import { NOW, statusFixture } from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  // The clock difference is recorded once per module: the next test starts without it.
  resetServerSkew();
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
  document.title = '';
});

async function open(path: string, role: Role, status: Responder | Responder[]) {
  window.history.replaceState(null, '', path);
  mock = mockFetch({
    'GET /session': { json: { name: 'anna', role } },
    'GET /status': status,
    // The page behind /hosts loads its own data.
    'GET /hosts': { json: { items: [], total: 0, metrics_available: true } },
    'GET /host-groups': { json: { items: [] } },
  });
  render(<App />);
  // The session answers first, then the shell mounts and asks for the status.
  await advance(0);
  await advance(0);
}

function problems(critical: number, warning: number): Status['problems'] {
  return {
    ...statusFixture().problems,
    critical: { open: critical, unacknowledged: critical },
    warning: { open: warning, unacknowledged: warning },
  };
}

describe('navigation', () => {
  it('shows the badge as the sum of unacknowledged problems', async () => {
    await open('/hosts', 'viewer', { json: statusFixture({ problems: problems(2, 1) }) });
    const link = screen.getByRole('link', { name: /Problems/ });
    expect(within(link).getByText('3')).toBeInTheDocument();
  });

  it('shows no badge at 0', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    const link = screen.getByRole('link', { name: /Problems/ });
    expect(link).toHaveTextContent(/^Problems$/);
  });

  it('hides Administration from an operator', async () => {
    await open('/hosts', 'operator', { json: statusFixture() });
    expect(screen.queryByRole('link', { name: 'Administration' })).not.toBeInTheDocument();
  });

  it('shows Administration to an admin', async () => {
    await open('/hosts', 'admin', { json: statusFixture() });
    expect(screen.getByRole('link', { name: 'Administration' })).toHaveAttribute('href', '/administration');
  });

  it('marks the current page, also for a page below it', async () => {
    await open('/hosts/db-02', 'viewer', { json: statusFixture() });
    expect(screen.getByRole('link', { name: 'Hosts' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('link', { name: 'Dashboards' })).not.toHaveAttribute('aria-current');
  });

  it('shows the user name and role', async () => {
    await open('/hosts', 'operator', { json: statusFixture() });
    expect(screen.getByText('anna · Operator')).toBeInTheDocument();
  });

  it('moves focus to the main region and sets the title on navigation', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    expect(document.title).toBe('Hosts · Cerbrix');
    screen.getByRole('link', { name: 'Problems' }).click();
    await advance(0);
    expect(screen.getByRole('heading', { level: 1, name: 'Problems' })).toBeInTheDocument();
    expect(document.title).toBe('Problems · Cerbrix');
    expect(screen.getByRole('main')).toHaveFocus();
  });

  it('sends / to the dashboards', async () => {
    await open('/', 'viewer', { json: statusFixture() });
    expect(window.location.pathname).toBe('/dashboards');
  });

  it('shows the not-found page inside the shell', async () => {
    await open('/nope', 'viewer', { json: statusFixture() });
    expect(screen.getByRole('heading', { level: 1, name: 'Page not found' })).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument();
  });

  it('decodes the host name of /hosts/:name for the placeholder heading', async () => {
    await open('/hosts/db%2D02', 'viewer', { json: statusFixture() });
    expect(screen.getByRole('heading', { level: 1, name: 'db-02' })).toBeInTheDocument();
  });
});

describe('role gating', () => {
  it('shows Not allowed for your role at /administration to an operator', async () => {
    await open('/administration', 'operator', { json: statusFixture() });
    expect(screen.getByText('Not allowed for your role')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Administration' })).not.toBeInTheDocument();
  });

  it('shows the page to an admin', async () => {
    await open('/administration', 'admin', { json: statusFixture() });
    expect(screen.getByRole('heading', { level: 1, name: 'Administration' })).toBeInTheDocument();
  });
});

const OUT_OF_SYNC = {
  status: 'out_of_sync' as const,
  active_commit: '253a2994b13aca387e272ff08db9eb858b436b66',
  last_success_at: new Date(2026, 10, 17, 14, 5, 0).toISOString(),
  last_attempt_at: null,
  issues: [] as Status['sync']['issues'],
};

describe('top bar texts for missing data', () => {
  it('says no configuration is active when out of sync without a commit', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({ sync: { ...OUT_OF_SYNC, active_commit: null } }),
    });
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Out of sync since 14:05, no configuration is active',
    );
    expect(screen.getByRole('alert')).not.toHaveTextContent('from commit');
  });

  it('leaves out never and since when nothing was ever applied', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({
        sync: { ...OUT_OF_SYNC, active_commit: null, last_success_at: null },
      }),
    });
    expect(screen.getByRole('banner')).toHaveTextContent(/^Out of sync(?! ·)/);
    expect(screen.getByRole('banner')).not.toHaveTextContent('never');
    const banner = screen.getByRole('alert');
    expect(banner).toHaveTextContent('Out of sync, no configuration is active');
    expect(banner).not.toHaveTextContent('since');
  });

  it('shows the first issue and the link for never with issues', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({
        sync: {
          status: 'never',
          active_commit: null,
          last_success_at: null,
          last_attempt_at: null,
          issues: [
            { file: 'a.json', line: 1, column: 2, reason: 'bad' },
            { file: null, line: null, column: null, reason: 'worse' },
          ],
        },
      }),
    });
    expect(screen.getByText('The configuration has not been synchronised yet')).toBeInTheDocument();
    const banner = screen.getByRole('alert');
    expect(banner).toHaveTextContent('a.json:1:2 — bad');
    expect(within(banner).getByRole('link', { name: 'and 1 more' })).toHaveAttribute(
      'href',
      '/configuration',
    );
  });

  it('says the server reported an error when the status poll fails with another error', async () => {
    await open('/hosts', 'viewer', [
      { json: statusFixture() },
      { status: 500, json: { error: 'internal', message: 'boom' } },
    ]);
    await advance(10000);
    expect(screen.getByRole('alert')).toHaveTextContent('The server reported an error since 14:30');
    expect(screen.queryByText(/No connection/)).not.toBeInTheDocument();
  });
});

describe('top bar', () => {
  it('shows the sync state with commit and age, and the note about Git', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    const bar = screen.getByRole('banner');
    expect(bar).toHaveTextContent('In sync · 253a299 · 2 min ago');
    expect(bar).toHaveTextContent('Hosts, templates and dashboards are managed by Git');
  });

  it('keeps the age up to date with the shell clock', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    await advance(10000);
    expect(screen.getByRole('banner')).toHaveTextContent('In sync · 253a299 · 2 min ago');
    await advance(60000);
    expect(screen.getByRole('banner')).toHaveTextContent('3 min ago');
  });

  it('shows no sync state and no Git note with gitops off', async () => {
    await open('/hosts', 'viewer', { json: statusFixture({ gitops: false }) });
    const bar = screen.getByRole('banner');
    expect(bar).not.toHaveTextContent('In sync');
    expect(bar).not.toHaveTextContent('managed by Git');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('says the configuration has not been synchronised yet', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({
        sync: { status: 'never', active_commit: null, last_success_at: null, last_attempt_at: null, issues: [] },
      }),
    });
    expect(screen.getByText('The configuration has not been synchronised yet')).toBeInTheDocument();
  });

  it('warns when out of sync, with the first issue, the rest counted, and a link to Configuration', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({
        sync: {
          status: 'out_of_sync',
          active_commit: '253a2994b13aca387e272ff08db9eb858b436b66',
          last_success_at: new Date(2026, 10, 17, 14, 5, 0).toISOString(),
          last_attempt_at: new Date(2026, 10, 17, 14, 29, 0).toISOString(),
          issues: [
            { file: 'dashboards/a.json', line: 12, column: 4, reason: 'unknown panel type' },
            { file: null, line: null, column: null, reason: 'second' },
            { file: 'hosts/b.json', line: null, column: null, reason: 'third' },
          ],
        },
      }),
    });
    const banner = screen.getByRole('alert');
    expect(banner).toHaveTextContent(
      'Out of sync since 14:05, showing the configuration from commit 253a299',
    );
    expect(banner).toHaveTextContent('dashboards/a.json:12:4 — unknown panel type');
    const more = within(banner).getByRole('link', { name: 'and 2 more' });
    expect(more).toHaveAttribute('href', '/configuration');
  });

  it('shows the text of an issue reason as text, never as markup', async () => {
    await open('/hosts', 'viewer', {
      json: statusFixture({
        sync: {
          status: 'out_of_sync',
          active_commit: '253a2994b13aca387e272ff08db9eb858b436b66',
          last_success_at: new Date(2026, 10, 17, 14, 5, 0).toISOString(),
          last_attempt_at: null,
          issues: [{ file: 'x.json', line: 1, column: 1, reason: '<script>alert(1)</script>' }],
        },
      }),
    });
    const banner = screen.getByRole('alert');
    expect(banner).toHaveTextContent('x.json:1:1 — <script>alert(1)</script>');
    expect(banner.querySelector('script')).toBeNull();
  });

  it('warns when the status poll has failed', async () => {
    await open('/hosts', 'viewer', [{ json: statusFixture() }, 'network']);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await advance(10000);
    expect(screen.getByText(/No connection to the server since/)).toHaveTextContent(
      'No connection to the server since 14:30',
    );
  });
});

const BOB_USER = {
  id: 'id-bob',
  name: 'bob',
  role: 'viewer' as const,
  disabled: false,
  created_at: new Date(2026, 10, 16, 21, 54).toISOString(),
  password_changed_at: new Date(2026, 10, 17, 12, 0).toISOString(),
};

describe('time limit of the status poll', () => {
  it('shows the connection banner for a status poll that never answers, and keeps polling', async () => {
    await open('/hosts', 'viewer', [{ json: statusFixture() }, () => new Promise(() => {}), { json: statusFixture() }]);
    await advance(10_000);
    expect(mock.count('GET /status')).toBe(2);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await advance(15_000);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
    await advance(10_000);
    expect(mock.count('GET /status')).toBe(3);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
  });
});

describe('log out', () => {
  const WARNING = 'Could not log out: the server did not answer. You are still logged in.';

  it.each([
    ['a network failure', 'network' as const],
    ['a 500', { status: 500, json: { error: 'internal', message: 'boom' } }],
  ])('stays in the shell with a warning after %s, until the next navigation', async (_what, answer) => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    mock.on('DELETE /session', answer);
    screen.getByRole('button', { name: 'Log out' }).click();
    await advance(0);
    expect(screen.getByRole('alert')).toHaveTextContent(WARNING);
    expect(screen.getByRole('heading', { level: 1, name: 'Hosts' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/hosts');
    screen.getByRole('link', { name: 'Dashboards' }).click();
    await advance(0);
    expect(screen.queryByText(WARNING)).not.toBeInTheDocument();
  });

  it('shows the warning for a logout that never answers, after the time limit', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    mock.on('DELETE /session', () => new Promise(() => {}));
    screen.getByRole('button', { name: 'Log out' }).click();
    await advance(30_000);
    expect(screen.getByText(WARNING)).toBeInTheDocument();
  });

  it('shows the login page for 204', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    mock.on('DELETE /session', { status: 204 });
    screen.getByRole('button', { name: 'Log out' }).click();
    await advance(0);
    expect(screen.getByLabelText('User name')).toBeInTheDocument();
    expect(screen.queryByText(WARNING)).not.toBeInTheDocument();
  });
});

describe('another user behind the cookie', () => {
  it('leaves nothing of the old user on screen after a 403 when /session names someone else', async () => {
    await open('/administration', 'admin', { json: statusFixture() });
    mock.on('GET /users', [
      { json: { items: [BOB_USER], total: 1 } },
      { status: 403, json: { error: 'forbidden', message: 'admin only' } },
    ]);
    screen.getByRole('link', { name: 'Hosts' }).click();
    await advance(0);
    screen.getByRole('link', { name: 'Administration' }).click();
    await advance(0);
    await advance(0);
    expect(screen.getByRole('table', { name: 'Users' })).toBeInTheDocument();
    mock.on('GET /session', { json: { name: 'bert', role: 'viewer' } });
    await advance(10_000);
    await advance(0);
    expect(screen.queryByRole('table', { name: 'Users' })).not.toBeInTheDocument();
    expect(screen.queryByText('bob')).not.toBeInTheDocument();
    expect(screen.getByText('Not allowed for your role')).toBeInTheDocument();
    expect(screen.getByText('bert · Viewer')).toBeInTheDocument();
    expect(screen.queryByText(/anna/)).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Administration' })).not.toBeInTheDocument();
    expect(window.location.pathname).toBe('/administration');
  });

  it('keeps the page as it is when the tab becomes visible and the user is the same', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    const input = screen.getByRole('searchbox', { name: 'Search' });
    input.focus();
    (input as HTMLInputElement).value = 'db';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    const before = mock.count('GET /session');
    document.dispatchEvent(new Event('visibilitychange'));
    await advance(0);
    expect(mock.count('GET /session')).toBe(before + 1);
    expect(screen.getByRole('searchbox', { name: 'Search' })).toHaveValue('db');
  });
});

describe('the note of a page and the banner of the shell', () => {
  it('shows the page note only while the status poll works', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    mock.on('GET /hosts', 'network');
    await advance(10_000);
    expect(screen.getByText(/^Not updated since/)).toHaveTextContent('Not updated since 14:30');
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    mock.on('GET /status', 'network');
    await advance(10_000);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
    expect(screen.queryByText(/^Not updated since/)).not.toBeInTheDocument();
  });
});

describe('the clock of the shell', () => {
  const FAST = new Date(NOW.getTime() + 5 * 60_000);

  it('follows the clock of the server when the browser clock is five minutes fast', async () => {
    vi.setSystemTime(FAST);
    await open('/hosts', 'viewer', { json: statusFixture(), headers: { Date: NOW.toUTCString() } });
    // The last synchronisation was at 14:28 on the server's clock.
    expect(screen.getByRole('banner')).toHaveTextContent('In sync · 253a299 · 2 min ago');
  });

  it('takes the browser clock when the answer has no Date header', async () => {
    vi.setSystemTime(FAST);
    await open('/hosts', 'viewer', { json: statusFixture() });
    expect(screen.getByRole('banner')).toHaveTextContent('In sync · 253a299 · 7 min ago');
  });

  it('moves on at once when the tab becomes visible again', async () => {
    await open('/hosts', 'viewer', { json: statusFixture() });
    expect(screen.getByRole('banner')).toHaveTextContent('2 min ago');
    // Time passes without a tick of the clock (a hidden tab's timers are slowed down).
    vi.setSystemTime(new Date(NOW.getTime() + 70_000));
    document.dispatchEvent(new Event('visibilitychange'));
    await advance(0);
    expect(screen.getByRole('banner')).toHaveTextContent('3 min ago');
  });
});

describe('top bar with GitOps off', () => {
  it('says where the configuration is managed', async () => {
    await open('/hosts', 'viewer', { json: statusFixture({ gitops: false }) });
    expect(screen.getByRole('banner')).toHaveTextContent('Configuration is managed on the server');
  });
});
