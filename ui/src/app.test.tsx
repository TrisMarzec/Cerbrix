import { fireEvent, render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { App } from './app';
import { dashboardDetailFixture, dashboardFixture, statusFixture } from './test/fixtures';
import { mockFetch, type FetchMock } from './test/mockFetch';
import { advance } from './test/timers';

const UNAUTHORIZED = { status: 401, json: { error: 'unauthorized', message: 'no valid session' } };

let mock: FetchMock;
afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
});

async function logIn(user: ReturnType<typeof userEvent.setup>) {
  await user.type(await screen.findByLabelText('User name'), 'anna');
  await user.type(screen.getByLabelText('Password'), 'secret-password');
  await user.click(screen.getByRole('button', { name: 'Log in' }));
}

describe('App', () => {
  it('shows only the page background while the first request is pending', () => {
    mock = mockFetch({ 'GET /session': () => new Promise(() => {}) });
    const { container } = render(<App />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the login page for 401, and the requested page after the login', async () => {
    window.history.replaceState(null, '', '/problems?state=resolved');
    mock = mockFetch({
      'GET /session': UNAUTHORIZED,
      'POST /session': { json: { name: 'anna', role: 'operator' } },
      'GET /status': { json: statusFixture() },
    });
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
    // A cold load without a session asks once: the 401 of the first check starts no re-check.
    expect(mock.count('GET /session')).toBe(1);
    await logIn(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Problems' })).toBeInTheDocument();
    expect(window.location.pathname + window.location.search).toBe('/problems?state=resolved');
    expect(mock.count('GET /session')).toBe(1);
  });

  it('shows the shell at once for a valid session', async () => {
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
    });
    render(<App />);
    expect(await screen.findByRole('heading', { level: 1, name: 'Dashboards' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/dashboards');
  });

  it('says it cannot reach the server on a network failure, and tries again', async () => {
    mock = mockFetch({ 'GET /session': ['network', { json: { name: 'anna', role: 'viewer' } }], 'GET /status': { json: statusFixture() } });
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByText('Could not reach the server')).toBeInTheDocument();
    expect(screen.queryByLabelText('User name')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Dashboards' })).toBeInTheDocument();
  });

  it('shows the login page when a later request is answered 401, and returns to the same URL', async () => {
    window.history.replaceState(null, '', '/problems?state=resolved');
    vi.useFakeTimers();
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, UNAUTHORIZED],
      'GET /status': [{ json: statusFixture() }, UNAUTHORIZED],
      'POST /session': { json: { name: 'anna', role: 'operator' } },
    });
    render(<App />);
    await advance(0);
    expect(screen.getByRole('heading', { level: 1, name: 'Problems' })).toBeInTheDocument();
    await advance(10000);
    expect(screen.getByText('Your session has ended. Log in again.')).toBeInTheDocument();
    expect(window.location.pathname + window.location.search).toBe('/problems?state=resolved');

    vi.useRealTimers();
    mock.on('GET /status', { json: statusFixture() });
    const user = userEvent.setup();
    await logIn(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Problems' })).toBeInTheDocument();
    expect(screen.queryByText('Your session has ended. Log in again.')).not.toBeInTheDocument();
    expect(window.location.pathname + window.location.search).toBe('/problems?state=resolved');
  });

  it('logs out, goes to / and shows the login page', async () => {
    window.history.replaceState(null, '', '/problems');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'operator' } },
      'GET /status': { json: statusFixture() },
      'DELETE /session': { status: 204 },
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('button', { name: 'Log out' }));
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/');
    expect(screen.queryByText('Your session has ended. Log in again.')).not.toBeInTheDocument();
  });
});

describe('App paths', () => {
  it('shows a dashboard in wall mode at /wall/<id>, without navigation and top bar', async () => {
    window.history.replaceState(null, '', '/wall/kunde-a');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
      'GET /dashboards/kunde-a': { json: dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' }) },
    });
    render(<App />);
    expect(await screen.findByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(mock.count('GET /dashboards/kunde-a')).toBe(1);
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Log out' })).not.toBeInTheDocument();
  });

  it('shows the login page for /wall/<id> without a session, and the wall after the login', async () => {
    window.history.replaceState(null, '', '/wall/kunde-a');
    mock = mockFetch({
      'GET /session': UNAUTHORIZED,
      'POST /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
      'GET /dashboards/kunde-a': { json: dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' }) },
    });
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
    expect(mock.count('GET /dashboards/kunde-a')).toBe(0);
    await logIn(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/wall/kunde-a');
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  });

  it('shows the wall display at /display/<id> without asking for the session', async () => {
    window.history.replaceState(null, '', '/display/x');
    mock = mockFetch({ 'GET /session': { json: { name: 'anna', role: 'viewer' } }, 'GET /display/x': UNAUTHORIZED });
    vi.useFakeTimers();
    render(<App />);
    await advance(100);
    expect(screen.getByRole('heading', { name: 'Wall display x' })).toBeInTheDocument();
    expect(screen.getByLabelText('Display token')).toBeInTheDocument();
    expect(mock.count('GET /display/x')).toBe(1);
    expect(mock.count('GET /session')).toBe(0);
  });

  it.each([
    ['a path below a wall', '/wall/a/b'],
    ['a malformed escape', '/wall/%E0%A4%A'],
  ])('shows the desk not-found page for %s', async (_, path) => {
    window.history.replaceState(null, '', path);
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
    });
    render(<App />);
    expect(await screen.findByRole('heading', { level: 1, name: 'Page not found' })).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument();
    expect(mock.requests.filter((r) => r.path.startsWith('/dashboards/'))).toHaveLength(0);
  });

  it('shows the login page in place when a wall poll is answered 401, and the wall again after the login', async () => {
    window.history.replaceState(null, '', '/wall/kunde-a');
    vi.useFakeTimers();
    const kundeA = dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' });
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'viewer' } }, UNAUTHORIZED],
      'GET /status': { json: statusFixture() },
      'GET /dashboards/kunde-a': [{ json: kundeA }, UNAUTHORIZED],
      'POST /session': { json: { name: 'anna', role: 'viewer' } },
    });
    render(<App />);
    await advance(0);
    await advance(0);
    expect(screen.getByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    await advance(5000);
    expect(screen.getByText('Your session has ended. Log in again.')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/wall/kunde-a');

    vi.useRealTimers();
    mock.on('GET /dashboards/kunde-a', { json: kundeA });
    const user = userEvent.setup();
    await logIn(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/wall/kunde-a');
  });

  it('shows the unreachable server on the dark wall background, and the wall without a click once it answers', async () => {
    window.history.replaceState(null, '', '/wall/kunde-a');
    vi.useFakeTimers();
    mock = mockFetch({
      'GET /session': ['network', { json: { name: 'anna', role: 'viewer' } }],
      'GET /status': { json: statusFixture() },
      'GET /dashboards/kunde-a': { json: dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' }) },
    });
    const { container } = render(<App />);
    // Before the session is known: the wall's background, nothing else.
    expect(container.firstElementChild).toHaveClass('wall');
    expect(container.firstElementChild).toBeEmptyDOMElement();
    await advance(0);
    const gate = screen.getByText('Could not reach the server');
    expect(gate.closest('.wall')).toBe(container.firstElementChild);
    expect(gate.closest('main')).toHaveClass('wall-gate');
    await advance(4999);
    expect(mock.count('GET /session')).toBe(1);
    await advance(1);
    await advance(0);
    expect(mock.count('GET /session')).toBe(2);
    expect(screen.getByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(screen.queryByText('Could not reach the server')).not.toBeInTheDocument();
  });

  it('shows the desk gate without the wall background on other paths', async () => {
    window.history.replaceState(null, '', '/hosts');
    mock = mockFetch({ 'GET /session': 'network' });
    const { container } = render(<App />);
    expect(await screen.findByText('Could not reach the server')).toBeInTheDocument();
    expect(container.querySelector('.wall')).toBeNull();
  });

  it('still shows the shell for the desk pages', async () => {
    window.history.replaceState(null, '', '/hosts');
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
      'GET /hosts': { json: { items: [], total: 0 } },
    });
    render(<App />);
    expect(await screen.findByRole('heading', { level: 1, name: 'Hosts' })).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument();
  });

  it('opens a dashboard on this screen from the dashboards page, and goes back to the desk', async () => {
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'viewer' } },
      'GET /status': { json: statusFixture() },
      'GET /dashboards': { json: { items: [dashboardFixture({ id: 'kunde-a', name: 'Kunde A' })] } },
      'GET /displays': { json: { items: [] } },
      'GET /dashboards/kunde-a': { json: dashboardDetailFixture({ id: 'kunde-a', name: 'Kunde A' }) },
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('link', { name: 'Open Kunde A on this screen' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/wall/kunde-a');
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(mock.count('GET /session')).toBe(1);
    await user.click(screen.getByRole('link', { name: 'Back to the desk' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Dashboards' })).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument();
  });
});

describe('App after a password change', () => {
  it('stays in the shell, with the notice, when a poll with the old cookie gets 401 but the session is alive', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 10, 17, 14, 30, 0));
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'operator' } },
      'GET /status': [{ json: statusFixture() }, UNAUTHORIZED, { json: statusFixture() }],
      'PUT /session/password': { status: 204 },
    });
    render(<App />);
    await advance(0);
    await advance(0);
    await advance(3000);
    fireEvent.click(screen.getByRole('button', { name: 'Change password' }));
    await advance(0);
    const fields: Array<[string, string]> = [
      ['Current password', 'old-password'],
      ['New password', 'a-long-new-password'],
      ['Repeat new password', 'a-long-new-password'],
    ];
    for (const [label, value] of fields) {
      fireEvent.input(screen.getByLabelText(label), { target: { value } });
    }
    await advance(0);
    fireEvent.submit(screen.getByLabelText('Current password').closest('form') as HTMLFormElement);
    await advance(0);
    expect(screen.getByText('Password changed')).toBeInTheDocument();
    await advance(7000);
    expect(mock.count('GET /status')).toBe(2);
    expect(mock.count('GET /session')).toBe(2);
    expect(screen.queryByText('Your session has ended. Log in again.')).not.toBeInTheDocument();
    expect(screen.getByText('Password changed')).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: 'Dashboards' })).toBeInTheDocument();
  });
});
