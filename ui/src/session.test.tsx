import { render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { api } from './api/client';
import { SessionProvider, useSession } from './session';
import { mockFetch, type Answer, type FetchMock } from './test/mockFetch';

const UNAUTHORIZED: Answer = { status: 401, json: { error: 'unauthorized', message: 'no valid session' } };

let mock: FetchMock;
afterEach(() => {
  mock.restore();
});

function Probe() {
  const { user, logout } = useSession();
  return (
    <>
      <p>
        user={user.name}/{user.role}
      </p>
      <button
        onClick={() => {
          void api.get('/slow').catch(() => {});
        }}
      >
        slow
      </button>
      <button
        onClick={() => {
          void Promise.all([1, 2, 3, 4, 5].map(() => api.get('/a').catch(() => {})));
        }}
      >
        five
      </button>
      <button
        onClick={() => {
          void logout();
        }}
      >
        out
      </button>
    </>
  );
}

describe('SessionProvider and 401', () => {
  it('asks GET /session once for five simultaneous 401s and stays in on a 200', async () => {
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, { json: { name: 'anna', role: 'viewer' } }],
      'GET /a': UNAUTHORIZED,
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('five'));
    // The role in the answer replaces the one the session had.
    expect(await screen.findByText('user=anna/viewer')).toBeInTheDocument();
    expect(mock.count('GET /a')).toBe(5);
    expect(mock.count('GET /session')).toBe(2);
  });

  it('shows the login page when the check is answered 401 too', async () => {
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, UNAUTHORIZED],
      'GET /a': UNAUTHORIZED,
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('five'));
    expect(await screen.findByText('Your session has ended. Log in again.')).toBeInTheDocument();
    expect(mock.count('GET /session')).toBe(2);
  });

  it('keeps the user in when the check fails on the network', async () => {
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, 'network'],
      'GET /a': UNAUTHORIZED,
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('five'));
    await vi.waitFor(() => {
      expect(mock.count('GET /session')).toBe(2);
    });
    expect(screen.getByText('user=anna/operator')).toBeInTheDocument();
  });

  it('ignores the 401 of a request of an earlier session', async () => {
    let finish: (a: Answer) => void = () => {};
    mock = mockFetch({
      'GET /session': { json: { name: 'anna', role: 'operator' } },
      'GET /slow': () =>
        new Promise<Answer>((resolve) => {
          finish = resolve;
        }),
      'DELETE /session': { status: 204 },
      'POST /session': { json: { name: 'bert', role: 'viewer' } },
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('slow'));
    await vi.waitFor(() => {
      expect(mock.count('GET /slow')).toBe(1);
    });
    await user.click(screen.getByText('out'));
    await user.type(await screen.findByLabelText('User name'), 'bert');
    await user.type(screen.getByLabelText('Password'), 'secret-password');
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    expect(await screen.findByText('user=bert/viewer')).toBeInTheDocument();
    finish(UNAUTHORIZED);
    // Let the answer arrive and be handled.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByText('user=bert/viewer')).toBeInTheDocument();
    expect(mock.count('GET /session')).toBe(1);
  });
});
