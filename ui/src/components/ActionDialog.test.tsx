import { fireEvent, render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { ApiError } from '../api/client';
import { ActionDialog } from './ActionDialog';

interface Gate {
  promise: Promise<string>;
  resolve: (v: string) => void;
  reject: (e: unknown) => void;
}

function gate(): Gate {
  let resolve: (v: string) => void = () => {};
  let reject: (e: unknown) => void = () => {};
  const promise = new Promise<string>((ok, fail) => {
    resolve = ok;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function show(props: Partial<Parameters<typeof ActionDialog<string>>[0]> = {}) {
  const onDone = vi.fn();
  const onClose = vi.fn();
  const run = vi.fn(() => Promise.resolve('ok'));
  render(
    <ActionDialog<string>
      title="Revoke key"
      submitLabel="Revoke"
      run={run}
      onDone={onDone}
      onClose={onClose}
      {...props}
    >
      <p>Revoke the key "x"?</p>
    </ActionDialog>,
  );
  return { onDone, onClose, run, user: userEvent.setup() };
}

describe('ActionDialog', () => {
  it('asks first and runs the request once on confirm', async () => {
    const { onDone, run, user } = show();
    expect(screen.getByRole('dialog', { name: 'Revoke key' })).toHaveTextContent('Revoke the key "x"?');
    expect(run).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(run).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledWith('ok');
  });

  it('cannot be dismissed and runs nothing twice while the request runs', async () => {
    const pending = gate();
    const run = vi.fn(() => pending.promise);
    const { onClose, user } = show({ run });
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    fireEvent.submit(screen.getByRole('button', { name: 'Revoke' }).closest('form') as HTMLFormElement);
    await user.keyboard('{Escape}');
    fireEvent.mouseDown(screen.getByRole('dialog').parentElement as HTMLElement);
    expect(onClose).not.toHaveBeenCalled();
    expect(run).toHaveBeenCalledTimes(1);
    pending.reject(new ApiError(0, 'network', 'x'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not reach the server.');
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('keeps a failure inside, with the sentence the caller gives for its status', async () => {
    const onFailed = vi.fn();
    const failure = new ApiError(404, 'not_found', 'no such key');
    const { onDone, user } = show({
      run: () => Promise.reject(failure),
      texts: { 404: 'The key is gone.' },
      onFailed,
    });
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The key is gone.');
    expect(onFailed).toHaveBeenCalledWith(failure);
    expect(onDone).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeEnabled();
  });

  it('sends nothing when the input is not acceptable', async () => {
    const { run, user } = show({ validate: () => 'Enter a name.' });
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a name.');
    expect(run).not.toHaveBeenCalled();
  });

  it('calls afterAttempt after a success and after a failure', async () => {
    const afterAttempt = vi.fn();
    const run = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(new ApiError(500, 'internal', 'boom'))
      .mockResolvedValue('ok');
    const { user } = show({ run, afterAttempt });
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    await screen.findByRole('alert');
    expect(afterAttempt).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(afterAttempt).toHaveBeenCalledTimes(2);
  });

  it('can keep the submit button disabled', () => {
    show({ submitDisabled: true });
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeDisabled();
  });
});
