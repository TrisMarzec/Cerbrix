import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, isApiError } from '../api/client';
import type { DisplayState } from '../api/types';
import { displayGet, storeToken, tokenKept } from './displayClient';
import { WallRoot } from './WallRoot';

/** How long the notice that the browser does not keep the token stays before the display starts. */
const NOTICE_MS = 10_000;

interface Props {
  id: string;
  /** Why the form is shown again, such as a token that stopped working. */
  message?: string | undefined;
  /** The token works and is stored: `state` is the display's state it read. */
  onStart: (state: DisplayState) => void;
}

function messageFor(e: unknown): string {
  const status = e instanceof ApiError ? e.status : 0;
  // Only the API's own 401 (R15): a gateway's says nothing about the token.
  if (isApiError(e, 'unauthorized')) return 'This token is not valid for this display.';
  if (status === 0) return 'No connection to the server.';
  return 'The server reported an error.';
}

/**
 * The display token, entered once on the monitor (concept section 5.2). The token is checked by
 * asking for the display's state with it; only a token that works is stored. The form is the only
 * thing on the wall that takes input; the field keeps the focus.
 */
export function TokenForm({ id, message, onStart }: Props) {
  const input = useRef<HTMLInputElement>(null);
  const [error, setError] = useState(message);
  const [busy, setBusy] = useState(false);
  /** The state read with a token this browser does not keep: the display starts after the notice. */
  const [unkept, setUnkept] = useState<DisplayState>();
  const start = useRef(onStart);
  start.current = onStart;
  const controller = useRef<AbortController | undefined>(undefined);

  useEffect(() => {
    // As attributes: Preact would set `spellcheck` as a property, which not every engine has (and
    // `false` would then remove the attribute), and `autocorrect` is not in its types.
    input.current?.setAttribute('spellcheck', 'false');
    input.current?.setAttribute('autocorrect', 'off');
    input.current?.focus();
    return () => controller.current?.abort();
  }, []);

  useEffect(() => {
    if (unkept === undefined) return undefined;
    const timer = setTimeout(() => start.current(unkept), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [unkept]);

  const submit = (event: Event) => {
    event.preventDefault();
    const token = (input.current?.value ?? '').trim();
    input.current?.focus();
    if (busy || unkept !== undefined || token === '') return;
    setBusy(true);
    const mine = new AbortController();
    controller.current = mine;
    displayGet<DisplayState>(id, '', mine.signal, token).then(
      (state) => {
        if (mine.signal.aborted) return;
        storeToken(id, token);
        // Out of the field before anything else is shown: the monitor must not show the token.
        if (input.current !== null) input.current.value = '';
        if (tokenKept(id)) {
          start.current(state);
        } else {
          setError(undefined);
          setUnkept(state);
        }
      },
      (e: unknown) => {
        if (mine.signal.aborted) return;
        setError(messageFor(e));
        setBusy(false);
        input.current?.focus();
      },
    );
  };

  return (
    <WallRoot>
      <main class="wall-token">
        <form class="wall-token-form" onSubmit={submit}>
          <h1 class="wall-token-title">Wall display {id}</h1>
          <label class="wall-token-label" for="wall-display-token">
            Display token
          </label>
          <input
            ref={input}
            id="wall-display-token"
            class="wall-token-input"
            // Not a password field (ruling R13): a kiosk browser would offer to save it after the
            // start, and that bubble would stay over the wall until someone clicks it.
            type="text"
            inputMode="text"
            autocomplete="off"
            autocapitalize="off"
          />
          <button class="btn wall-token-button" type="submit" disabled={busy || unkept !== undefined}>
            Start
          </button>
          {error !== undefined && (
            <p class="wall-token-message" role="alert">
              {error}
            </p>
          )}
          {unkept !== undefined && (
            <p class="wall-token-notice" role="status">
              This browser does not keep the token. It must be entered again after a restart.
            </p>
          )}
        </form>
      </main>
    </WallRoot>
  );
}
