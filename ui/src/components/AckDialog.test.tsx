import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { criticalProblem } from '../test/fixtures';
import { mockFetch, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { AckDialog } from './AckDialog';

let mock: FetchMock;

afterEach(() => {
  mock.restore();
});

const acknowledged = {
  ...criticalProblem,
  acknowledged_at: '2026-11-17T13:00:00Z',
  acknowledged_by: 'anna',
};

function setup(answer: Responder) {
  mock = mockFetch({ [`POST /problems/${criticalProblem.id}/ack`]: answer });
  const onDone = vi.fn();
  const onClose = vi.fn();
  render(<AckDialog problem={criticalProblem} onDone={onDone} onClose={onClose} />);
  return { onDone, onClose, user: userEvent.setup() };
}

describe('AckDialog', () => {
  it('names the problem it is about', () => {
    setup({ json: acknowledged });
    expect(screen.getByText('db-01')).toBeInTheDocument();
    expect(screen.getByText(/Disk almost full · mount=\//)).toBeInTheDocument();
    expect(screen.getByText('disk.used_percent > 90: value 94.2')).toBeInTheDocument();
  });

  it('limits the comment to 1000 characters', () => {
    setup({ json: acknowledged });
    expect(screen.getByLabelText('Comment (optional)')).toHaveAttribute('maxlength', '1000');
  });

  it('sends an empty body without a comment and hands the answer to onDone', async () => {
    const { user, onDone } = setup({ json: acknowledged });
    await user.click(screen.getByRole('button', { name: 'Acknowledge' }));
    await waitFor(() => {
      expect(onDone).toHaveBeenCalledWith(acknowledged);
    });
    expect(mock.requests).toHaveLength(1);
    expect(mock.requests[0]?.body).toEqual({});
  });

  it('sends the comment when there is one', async () => {
    const { user, onDone } = setup({ json: acknowledged });
    await user.type(screen.getByLabelText('Comment (optional)'), 'disk replaced');
    await user.click(screen.getByRole('button', { name: 'Acknowledge' }));
    await waitFor(() => {
      expect(onDone).toHaveBeenCalled();
    });
    expect(mock.requests[0]?.body).toEqual({ comment: 'disk replaced' });
  });

  it('disables the button while the request runs, so it is sent once', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const { user, onDone } = setup(async () => {
      await gate;
      return { json: acknowledged };
    });
    const button = screen.getByRole('button', { name: 'Acknowledge' });
    await user.click(button);
    await waitFor(() => {
      expect(button).toBeDisabled();
    });
    await user.click(button);
    release();
    await waitFor(() => {
      expect(onDone).toHaveBeenCalledTimes(1);
    });
    expect(mock.requests).toHaveLength(1);
  });

  it.each([
    [409, { error: 'conflict', message: 'resolved' }, 'This problem is already resolved.'],
    [403, { error: 'forbidden', message: 'no' }, 'Your role may not do this.'],
    [
      429,
      { error: 'rate_limited', message: 'Too many requests, wait a moment' },
      'The server is busy, try again in a moment.',
    ],
    [500, { error: 'internal', message: 'Something broke' }, 'Something broke'],
  ])('keeps the dialog open on %i and says why', async (status, json, text) => {
    const { user, onDone, onClose } = setup({ status, json });
    await user.click(screen.getByRole('button', { name: 'Acknowledge' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(text);
    expect(onDone).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Acknowledge' })).toBeEnabled();
  });

  it('says so when the server cannot be reached', async () => {
    const { user } = setup('network');
    await user.click(screen.getByRole('button', { name: 'Acknowledge' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not reach the server.');
  });

  it('cancels without a request', async () => {
    const { user, onClose } = setup({ json: acknowledged });
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalled();
    expect(mock.requests).toHaveLength(0);
  });

  it('cannot be dismissed while the request runs', async () => {
    const { user, onClose } = setup(() => new Promise<never>(() => {}));
    
    await user.click(screen.getByRole('button', { name: 'Acknowledge' }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    });
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe('AckDialog and a request that never answers', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('says it could not reach the server after the time limit and can be dismissed again', async () => {
    vi.useFakeTimers();
    const { onClose } = setup(() => new Promise(() => {}));
    fireEvent.click(screen.getByRole('button', { name: 'Acknowledge' }));
    await advance(0);
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    await advance(30_000);
    expect(screen.getByRole('alert')).toHaveTextContent('Could not reach the server.');
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
