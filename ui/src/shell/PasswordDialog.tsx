import { useState } from 'preact/hooks';

import { api, ApiError } from '../api/client';
import { Dialog } from '../components/Dialog';

interface Props {
  /** The account the password belongs to; password managers use it to pick the right entry. */
  userName?: string;
  onClose: () => void;
  /** Called after the server accepted the new password; the dialog is closed by the caller. */
  onChanged: () => void;
}

const MIN = 12;
const MAX = 256;

function messageFor(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.status === 403) return 'The current password is wrong';
    if (e.status === 429) return 'The server is busy, try again in a moment';
    if (e.status !== 0) return e.message;
  }
  return 'Could not reach the server';
}

export function PasswordDialog({ userName, onClose, onChanged }: Props) {
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    if (busy) return;
    const length = [...next].length;
    if (length < MIN || length > MAX) {
      setError(`The new password must have ${MIN} to ${MAX} characters`);
      return;
    }
    if (next !== repeat) {
      setError('The two new passwords are not the same');
      return;
    }
    const body = { current, new: next };
    setError(undefined);
    setBusy(true);
    try {
      await api.send('PUT', '/session/password', body);
      onChanged();
    } catch (failure) {
      setError(messageFor(failure));
    } finally {
      // Passwords live only as long as the open form and the request.
      setCurrent('');
      setNext('');
      setRepeat('');
      setBusy(false);
    }
  }

  return (
    <Dialog title="Change password" onClose={onClose}>
      <form class="dialog-form" onSubmit={(e) => void submit(e)}>
        <label class="field">
          Current password
          <input
            type="password"
            autocomplete="current-password"
            value={current}
            onInput={(e) => {
              setCurrent(e.currentTarget.value);
            }}
          />
        </label>
        <label class="field">
          New password
          <input
            type="password"
            autocomplete="new-password"
            value={next}
            onInput={(e) => {
              setNext(e.currentTarget.value);
            }}
          />
        </label>
        <label class="field">
          Repeat new password
          <input
            type="password"
            autocomplete="new-password"
            value={repeat}
            onInput={(e) => {
              setRepeat(e.currentTarget.value);
            }}
          />
        </label>
        {userName !== undefined && (
          <input
            type="text"
            class="visually-hidden"
            readOnly
            tabIndex={-1}
            aria-hidden="true"
            autocomplete="username"
            value={userName}
          />
        )}
        <p class="dim">
          <small>
            {MIN} to {MAX} characters.
          </small>
        </p>
        {error !== undefined && (
          <p role="alert" class="form-error">
            {error}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy}>
            Change password
          </button>
        </div>
      </form>
    </Dialog>
  );
}
