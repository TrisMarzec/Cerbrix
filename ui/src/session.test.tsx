import { act, fireEvent, render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { api } from './api/client';
import { SessionProvider, useSession } from './session';
import { mockFetch, type Answer, type FetchMock } from './test/mockFetch';
import { advance } from './test/timers';

const UNAUTHORIZED: Answer = { status: 401, json: { error: 'unauthorized', message: 'no valid session' } };

/** Lets the promises that are ready (a mocked answer and its handlers) finish; no timer involved. */
async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 20; i++) await Promise.resolve();
  });
}

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
    await flush();
    expect(screen.getByText('user=bert/viewer')).toBeInTheDocument();
    expect(mock.count('GET /session')).toBe(1);
  });
});

describe('SessionProvider first check', () => {
  it('asks GET /session exactly once for a visitor without a session', async () => {
    mock = mockFetch({ 'GET /session': UNAUTHORIZED });
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
    await flush();
    expect(mock.count('GET /session')).toBe(1);
    expect(screen.queryByText('Your session has ended. Log in again.')).not.toBeInTheDocument();
  });
});

describe('SessionProvider logout', () => {
  async function logOut(answer: Answer) {
    mock = mockFetch({ 'GET /session': { json: { name: 'anna', role: 'operator' } }, 'DELETE /session': answer });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('out'));
    await vi.waitFor(() => {
      expect(mock.count('DELETE /session')).toBe(1);
    });
    await flush();
  }

  it('stays in when the logout fails on the network', async () => {
    await logOut('network');
    expect(screen.getByText('user=anna/operator')).toBeInTheDocument();
    expect(screen.queryByLabelText('User name')).not.toBeInTheDocument();
  });

  it('stays in when the server answers the logout with 500', async () => {
    await logOut({ status: 500, json: { error: 'internal', message: 'boom' } });
    expect(screen.getByText('user=anna/operator')).toBeInTheDocument();
  });

  it('leaves for 204', async () => {
    await logOut({ status: 204 });
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
  });

  it('leaves for 401: the session was gone already', async () => {
    await logOut(UNAUTHORIZED);
    expect(await screen.findByLabelText('User name')).toBeInTheDocument();
    expect(screen.queryByText('Your session has ended. Log in again.')).not.toBeInTheDocument();
  });
});

function Typed() {
  const { user } = useSession();
  return (
    <label>
      Note of {user.name}
      <input />
    </label>
  );
}

describe('SessionProvider and another user behind the cookie', () => {
  let hidden = false;
  beforeEach(() => {
    hidden = false;
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
  });
  function setHidden(value: boolean) {
    hidden = value;
    document.dispatchEvent(new Event('visibilitychange'));
  }

  it('asks once who is logged in when the tab becomes visible again', async () => {
    mock = mockFetch({ 'GET /session': { json: { name: 'anna', role: 'operator' } } });
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await screen.findByText('user=anna/operator');
    expect(mock.count('GET /session')).toBe(1);
    setHidden(true);
    expect(mock.count('GET /session')).toBe(1);
    setHidden(false);
    await vi.waitFor(() => {
      expect(mock.count('GET /session')).toBe(2);
    });
  });

  it('asks once who is logged in when the window gets focus', async () => {
    mock = mockFetch({ 'GET /session': { json: { name: 'anna', role: 'operator' } } });
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await screen.findByText('user=anna/operator');
    expect(mock.count('GET /session')).toBe(1);
    window.dispatchEvent(new Event('focus'));
    await flush();
    expect(mock.count('GET /session')).toBe(2);
  });

  it('asks once for a focus and a visibility change arriving together', async () => {
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, () => new Promise<Answer>(() => {})],
    });
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await screen.findByText('user=anna/operator');
    window.dispatchEvent(new Event('focus'));
    setHidden(false);
    await flush();
    expect(mock.count('GET /session')).toBe(2);
  });

  it('stops listening for focus when it unmounts', async () => {
    mock = mockFetch({ 'GET /session': { json: { name: 'anna', role: 'operator' } } });
    const { unmount } = render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await screen.findByText('user=anna/operator');
    await act(async () => {
      unmount();
    });
    window.dispatchEvent(new Event('focus'));
    await flush();
    expect(mock.count('GET /session')).toBe(1);
  });

  it('asks once for several 403 answers and a visibility change together', async () => {
    let finish: (a: Answer) => void = () => {};
    mock = mockFetch({
      'GET /session': [
        { json: { name: 'anna', role: 'operator' } },
        () =>
          new Promise<Answer>((resolve) => {
            finish = resolve;
          }),
      ],
      'GET /a': { status: 403, json: { error: 'forbidden', message: 'no' } },
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
    setHidden(false);
    await flush();
    expect(mock.count('GET /a')).toBe(5);
    expect(mock.count('GET /session')).toBe(2);
    finish({ json: { name: 'anna', role: 'operator' } });
    await flush();
    expect(screen.getByText('user=anna/operator')).toBeInTheDocument();
  });

  it('remounts everything below it for another user, keeps it for the same one', async () => {
    mock = mockFetch({
      'GET /session': [
        { json: { name: 'anna', role: 'admin' } },
        { json: { name: 'anna', role: 'admin' } },
        { json: { name: 'bert', role: 'viewer' } },
      ],
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Typed />
      </SessionProvider>,
    );
    await user.type(await screen.findByLabelText('Note of anna'), 'kept');
    setHidden(false);
    await vi.waitFor(() => {
      expect(mock.count('GET /session')).toBe(2);
    });
    await flush();
    expect(screen.getByLabelText('Note of anna')).toHaveValue('kept');
    setHidden(false);
    expect(await screen.findByLabelText('Note of bert')).toHaveValue('');
  });

  it('starts a new session generation for another user, so a late 401 of the old one is ignored', async () => {
    let finish: (a: Answer) => void = () => {};
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'admin' } }, { json: { name: 'bert', role: 'viewer' } }],
      'GET /slow': () =>
        new Promise<Answer>((resolve) => {
          finish = resolve;
        }),
    });
    const user = userEvent.setup();
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await user.click(await screen.findByText('slow'));
    setHidden(false);
    expect(await screen.findByText('user=bert/viewer')).toBeInTheDocument();
    finish(UNAUTHORIZED);
    await flush();
    expect(mock.count('GET /session')).toBe(2);
    expect(screen.getByText('user=bert/viewer')).toBeInTheDocument();
  });
});

describe('SessionProvider and a check that never answers', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('gives the re-check up after the time limit, so the next 401 checks again', async () => {
    vi.useFakeTimers();
    mock = mockFetch({
      'GET /session': [{ json: { name: 'anna', role: 'operator' } }, () => new Promise<Answer>(() => {}), { json: { name: 'anna', role: 'operator' } }],
      'GET /a': UNAUTHORIZED,
    });
    render(
      <SessionProvider>
        <Probe />
      </SessionProvider>,
    );
    await advance(0);
    fireEvent.click(screen.getByText('five'));
    await advance(0);
    expect(mock.count('GET /session')).toBe(2);
    fireEvent.click(screen.getByText('five'));
    await advance(0);
    expect(mock.count('GET /session')).toBe(2);
    await advance(15_000);
    fireEvent.click(screen.getByText('five'));
    await advance(0);
    expect(mock.count('GET /session')).toBe(3);
    expect(screen.getByText('user=anna/operator')).toBeInTheDocument();
  });
});
