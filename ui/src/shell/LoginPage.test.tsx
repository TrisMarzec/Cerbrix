import { render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { mockFetch, type FetchMock } from '../test/mockFetch';
import { LoginPage } from './LoginPage';

let mock: FetchMock;
afterEach(() => {
  mock.restore();
});

async function fill(user: ReturnType<typeof userEvent.setup>, password = 'secret-password') {
  await user.type(screen.getByLabelText('User name'), 'anna');
  await user.type(screen.getByLabelText('Password'), password);
}

describe('LoginPage', () => {
  it('posts the name and password and hands the user on', async () => {
    mock = mockFetch({ 'POST /session': { json: { name: 'anna', role: 'operator' } } });
    const onLoggedIn = vi.fn();
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={onLoggedIn} />);
    await fill(user);
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    await vi.waitFor(() => {
      expect(onLoggedIn).toHaveBeenCalledWith({ name: 'anna', role: 'operator' });
    });
    expect(mock.requests[0]?.body).toEqual({ name: 'anna', password: 'secret-password' });
  });

  it('works with Enter', async () => {
    mock = mockFetch({ 'POST /session': { json: { name: 'anna', role: 'viewer' } } });
    const onLoggedIn = vi.fn();
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={onLoggedIn} />);
    await user.type(screen.getByLabelText('User name'), 'anna');
    await user.type(screen.getByLabelText('Password'), 'secret-password{enter}');
    await vi.waitFor(() => {
      expect(onLoggedIn).toHaveBeenCalledTimes(1);
    });
  });

  it('shows the message of the server for 401 and empties the password', async () => {
    mock = mockFetch({
      'POST /session': { status: 401, json: { error: 'unauthorized', message: 'wrong user name or password' } },
    });
    const onLoggedIn = vi.fn();
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={onLoggedIn} />);
    await fill(user);
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    expect(await screen.findByText('wrong user name or password')).toBeInTheDocument();
    expect(screen.getByLabelText('Password')).toHaveValue('');
    expect(screen.getByLabelText('User name')).toHaveValue('anna');
    expect(onLoggedIn).not.toHaveBeenCalled();
  });

  // A 429 at the login is a busy server or an address with too many failed logins: one
  // sentence that is true for both.
  it.each([
    'the server is busy checking passwords, try again shortly',
    'too many failed logins from this address; try again in a few minutes',
  ])('says to wait a few minutes for 429 (%s)', async (message) => {
    mock = mockFetch({
      'POST /session': { status: 429, json: { error: 'too_many_attempts', message } },
    });
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={() => {}} />);
    await fill(user);
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    expect(
      await screen.findByText('Too many attempts. Try again in a few minutes.'),
    ).toBeInTheDocument();
    expect(screen.getByLabelText('Password')).toHaveValue('');
  });

  it('says the server cannot be reached for any other failure', async () => {
    mock = mockFetch({ 'POST /session': 'network' });
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={() => {}} />);
    await fill(user);
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    expect(await screen.findByText('Could not reach the server.')).toBeInTheDocument();
  });

  it("shows the server's message for another failure", async () => {
    mock = mockFetch({
      'POST /session': { status: 500, json: { error: 'internal', message: 'The database is down.' } },
    });
    const user = userEvent.setup();
    render(<LoginPage expired={false} onLoggedIn={() => {}} />);
    await fill(user);
    await user.click(screen.getByRole('button', { name: 'Log in' }));
    expect(await screen.findByText('The database is down.')).toBeInTheDocument();
  });

  it('shows why the user is here after the session ended, and links the licences', () => {
    mock = mockFetch();
    render(<LoginPage expired={true} onLoggedIn={() => {}} />);
    expect(screen.getByText('Your session has ended. Log in again.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Licences' })).toHaveAttribute('href', '/licenses.txt');
  });
});
