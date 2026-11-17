import { fireEvent, render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { mockFetch, type Answer, type FetchMock } from '../test/mockFetch';
import { PasswordDialog } from './PasswordDialog';

let mock: FetchMock;
afterEach(() => {
  mock.restore();
});

const GOOD = 'a-long-new-password';

async function fill(
  user: ReturnType<typeof userEvent.setup>,
  values: { current?: string; next?: string; repeat?: string },
) {
  await user.type(screen.getByLabelText('Current password'), values.current ?? 'old-password');
  await user.type(screen.getByLabelText('New password'), values.next ?? GOOD);
  await user.type(screen.getByLabelText('Repeat new password'), values.repeat ?? GOOD);
}

describe('PasswordDialog', () => {
  it('refuses a new password that is too short without a request', async () => {
    mock = mockFetch();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={() => {}} onChanged={() => {}} />);
    await fill(user, { next: 'short', repeat: 'short' });
    await user.click(screen.getByRole('button', { name: 'Change password' }));
    expect(screen.getByText('The new password must have 12 to 256 characters')).toBeInTheDocument();
    expect(mock.requests).toHaveLength(0);
  });

  it('refuses two passwords that differ without a request', async () => {
    mock = mockFetch();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={() => {}} onChanged={() => {}} />);
    await fill(user, { repeat: 'another-long-password' });
    await user.click(screen.getByRole('button', { name: 'Change password' }));
    expect(screen.getByText('The two new passwords are not the same')).toBeInTheDocument();
    expect(mock.requests).toHaveLength(0);
  });

  it('keeps the dialog open with a message for 403 and clears the fields', async () => {
    mock = mockFetch({
      'PUT /session/password': { status: 403, json: { error: 'forbidden', message: 'the current password is wrong' } },
    });
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={() => {}} onChanged={onChanged} />);
    await fill(user, {});
    await user.click(screen.getByRole('button', { name: 'Change password' }));
    expect(await screen.findByText('The current password is wrong')).toBeInTheDocument();
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    for (const label of ['Current password', 'New password', 'Repeat new password']) {
      expect(screen.getByLabelText(label)).toHaveValue('');
    }
    expect(onChanged).not.toHaveBeenCalled();
    expect(mock.requests[0]?.body).toEqual({ current: 'old-password', new: GOOD });
  });

  it('reports success to the caller', async () => {
    mock = mockFetch({ 'PUT /session/password': { status: 204 } });
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={() => {}} onChanged={onChanged} />);
    await fill(user, {});
    await user.click(screen.getByRole('button', { name: 'Change password' }));
    await vi.waitFor(() => {
      expect(onChanged).toHaveBeenCalledTimes(1);
    });
  });

  it('closes on Cancel', async () => {
    mock = mockFetch();
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={onClose} onChanged={() => {}} />);
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('PasswordDialog while the request runs', () => {
  it('cannot be dismissed until the answer arrives', async () => {
    let finish: (a: Answer) => void = () => {};
    mock = mockFetch({
      'PUT /session/password': () =>
        new Promise<Answer>((resolve) => {
          finish = resolve;
        }),
    });
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(<PasswordDialog onClose={onClose} onChanged={() => {}} />);
    await fill(user, {});
    await user.click(screen.getByRole('button', { name: 'Change password' }));
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    await user.keyboard('{Escape}');
    const backdrop = screen.getByRole('dialog').parentElement as HTMLElement;
    fireEvent.mouseDown(backdrop);
    expect(onClose).not.toHaveBeenCalled();
    finish({ status: 500, json: { error: 'internal', message: 'The server failed.' } });
    expect(await screen.findByRole('alert')).toHaveTextContent('The server failed.');
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('PasswordDialog and password managers', () => {
  it('holds the user name in a hidden read-only field and focuses the first real field', () => {
    mock = mockFetch();
    render(<PasswordDialog userName="anna" onClose={() => {}} onChanged={() => {}} />);
    const hidden = screen.getByDisplayValue('anna');
    expect(hidden).toHaveAttribute('autocomplete', 'username');
    expect(hidden).toHaveAttribute('readonly');
    expect(hidden).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByLabelText('Current password')).toHaveFocus();
  });
});
