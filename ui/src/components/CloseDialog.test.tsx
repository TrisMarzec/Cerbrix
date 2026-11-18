import { render, screen, waitFor } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { criticalProblem } from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { CloseDialog } from './CloseDialog';

let mock: FetchMock;

afterEach(() => {
  mock.restore();
});

const closed = { ...criticalProblem, resolved_at: '2026-11-17T13:00:00Z' };

function setup(answer: Responder) {
  mock = mockFetch({ [`POST /problems/${criticalProblem.id}/close`]: answer });
  const onDone = vi.fn();
  const onClose = vi.fn();
  render(<CloseDialog problem={criticalProblem} onDone={onDone} onClose={onClose} />);
  return { onDone, onClose, user: userEvent.setup() };
}

describe('CloseDialog', () => {
  it('says what closing means and names the problem', () => {
    setup({ json: closed });
    expect(screen.getByText(/Closing ends this problem by hand\./)).toHaveTextContent(
      'Closing ends this problem by hand. Use it when the thing it is about no longer exists, for example a removed disk. If the condition still holds, a new problem opens with the next value.',
    );
    expect(screen.getByText('db-01')).toBeInTheDocument();
  });

  it('refuses an empty reason without a request', async () => {
    const { user, onDone } = setup({ json: closed });
    await user.type(screen.getByLabelText('Reason'), '   ');
    await user.click(screen.getByRole('button', { name: 'Close problem' }));
    expect(screen.getByRole('alert')).toHaveTextContent('A reason is required.');
    expect(mock.requests).toHaveLength(0);
    expect(onDone).not.toHaveBeenCalled();
  });

  it('sends the reason and hands the answer to onDone', async () => {
    const { user, onDone } = setup({ json: closed });
    await user.type(screen.getByLabelText('Reason'), 'disk removed');
    await user.click(screen.getByRole('button', { name: 'Close problem' }));
    await waitFor(() => {
      expect(onDone).toHaveBeenCalledWith(closed);
    });
    expect(mock.requests).toHaveLength(1);
    expect(mock.requests[0]?.body).toEqual({ comment: 'disk removed' });
  });

  it.each([
    [409, { error: 'conflict', message: 'resolved' }, 'This problem is already resolved.'],
    [403, { error: 'forbidden', message: 'no' }, 'Your role may not do this.'],
    [500, { error: 'internal', message: 'Something broke' }, 'Something broke'],
  ])('keeps the dialog open on %i', async (status, json, text) => {
    const { user, onDone } = setup({ status, json });
    await user.type(screen.getByLabelText('Reason'), 'gone');
    await user.click(screen.getByRole('button', { name: 'Close problem' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(text);
    expect(onDone).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Close problem' })).toBeEnabled();
  });

  it('says so when the server cannot be reached', async () => {
    const { user } = setup('network');
    await user.type(screen.getByLabelText('Reason'), 'gone');
    await user.click(screen.getByRole('button', { name: 'Close problem' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not reach the server.');
  });

  it('cannot be dismissed while the request runs', async () => {
    const { user, onClose } = setup(() => new Promise<never>(() => {}));
    await user.type(screen.getByLabelText('Reason'), 'gone');
    await user.click(screen.getByRole('button', { name: 'Close problem' }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    });
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).not.toHaveBeenCalled();
  });
});
