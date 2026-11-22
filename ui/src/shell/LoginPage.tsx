import { useEffect, useRef, useState } from 'preact/hooks';

import { api } from '../api/client';
import type { SessionUser } from '../api/types';
import { describeError } from '../components/describeError';

interface Props {
  /** True when the user was logged in and the session has ended. */
  expired: boolean;
  onLoggedIn: (user: SessionUser) => void;
}

export function LoginPage({ expired, onLoggedIn }: Props) {
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const nameField = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameField.current?.focus();
  }, []);

  async function submit(e: Event) {
    e.preventDefault();
    if (busy) return;
    const attempt = password;
    // The password never stays in the form after an attempt, whatever the answer.
    setPassword('');
    setError(undefined);
    setBusy(true);
    try {
      onLoggedIn(await api.send<SessionUser>('POST', '/session', { name, password: attempt }));
    } catch (failure) {
      // A 401 (a wrong name or password) is said in the server's words. A 429 is a busy
      // server or an address with too many failed logins; the sentence fits both.
      setError(describeError(failure, { 429: 'Too many attempts. Try again in a few minutes.' }));
      setBusy(false);
    }
  }

  return (
    <main class="login">
      <form class="login-box panel" onSubmit={(e) => void submit(e)}>
        <h1>Cerbrix</h1>
        {expired && <p role="status">Your session has ended. Log in again.</p>}
        <label class="field">
          User name
          <input
            ref={nameField}
            name="name"
            autocomplete="username"
            value={name}
            onInput={(e) => {
              setName(e.currentTarget.value);
            }}
          />
        </label>
        <label class="field">
          Password
          <input
            name="password"
            type="password"
            autocomplete="current-password"
            value={password}
            onInput={(e) => {
              setPassword(e.currentTarget.value);
            }}
          />
        </label>
        {error !== undefined && (
          <p role="alert" class="form-error">
            {error}
          </p>
        )}
        <button type="submit" class="btn" disabled={busy}>
          Log in
        </button>
        <p>
          <small>
            <a href="/licenses.txt">Licences</a>
          </small>
        </p>
      </form>
    </main>
  );
}
