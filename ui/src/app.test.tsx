import { render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { App } from './app';
import { statusFixture } from './test/fixtures';
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
    await logIn(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Problems' })).toBeInTheDocument();
    expect(window.location.pathname + window.location.search).toBe('/problems?state=resolved');
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
      'GET /session': { json: { name: 'anna', role: 'operator' } },
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
