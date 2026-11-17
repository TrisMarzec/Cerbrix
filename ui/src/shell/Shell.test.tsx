import { render, screen, within } from '@testing-library/preact';

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
    expect(screen.getByText('anna, operator')).toBeInTheDocument();
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

  it('shows no sync text with gitops off', async () => {
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
