import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, READ_LIMIT_MS } from '../api/client';
import type { DisplayState } from '../api/types';
import { displayGet, forgetToken, storedToken } from './displayClient';

/** How often a wall asks the server (D3). */
export const POLL_MS = 5000;
/** How often a wall looks for a new version of the UI (D7). */
const VERSION_MS = 60_000;

export interface DisplayPoll {
  /** The last state the server gave; kept while later polls fail. */
  state: DisplayState | undefined;
  /** The error of the last poll; undefined after a success. */
  error: ApiError | undefined;
  /** Since when the polls fail, on the browser's clock; undefined after a success. */
  staleSince: Date | undefined;
}

function asApiError(e: unknown): ApiError {
  return e instanceof ApiError ? e : new ApiError(0, 'internal', 'Unexpected error');
}

/**
 * Polls the state of a display while `active`: at once, or, with `seed` (the answer the token form
 * just got), 5 seconds after it; then 5 seconds after each answer, so one request at most is in
 * flight. A failure keeps the last state and never stops the loop, except a 401: the stored token
 * the request carried is forgotten, `onUnauthorized(withToken)` is called and the loop stops.
 * Inactive, or on unmount, the request in flight is aborted and no timer is left.
 */
export function useDisplayState(
  id: string,
  active: boolean,
  seed: DisplayState | undefined,
  onUnauthorized: (withToken: boolean) => void,
): DisplayPoll {
  const [poll, setPoll] = useState<DisplayPoll>({ state: undefined, error: undefined, staleSince: undefined });
  const seedRef = useRef(seed);
  seedRef.current = seed;
  const unauthorized = useRef(onUnauthorized);
  unauthorized.current = onUnauthorized;

  useEffect(() => {
    if (!active) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;

    const schedule = () => {
      if (!stopped) timer = setTimeout(run, POLL_MS);
    };

    function run() {
      const mine = new AbortController();
      controller = mine;
      const sent = storedToken(id);
      displayGet<DisplayState>(id, '', mine.signal, sent).then(
        (state) => {
          if (mine.signal.aborted) return;
          setPoll({ state, error: undefined, staleSince: undefined });
          schedule();
        },
        (e: unknown) => {
          if (mine.signal.aborted) return;
          const error = asApiError(e);
          if (error.status === 401) {
            // Only the token this request carried: one entered meanwhile is not touched.
            if (sent !== undefined && storedToken(id) === sent) forgetToken(id);
            unauthorized.current(sent !== undefined);
            return;
          }
          setPoll((previous) => ({
            state: previous.state,
            error,
            staleSince: previous.staleSince ?? new Date(),
          }));
          schedule();
        },
      );
    }

    const start = seedRef.current;
    if (start !== undefined) {
      setPoll({ state: start, error: undefined, staleSince: undefined });
      schedule();
    } else {
      run();
    }
    return () => {
      stopped = true;
      clearTimeout(timer);
      controller?.abort();
    };
  }, [id, active]);

  return poll;
}

/** Set once a reload was asked for: a page reloads itself once at most (D7). */
let reloadAsked = false;

/** Lets the next page reload again; for tests. */
export function resetVersionCheck(): void {
  reloadAsked = false;
}

/**
 * Looks for a new version of the UI (D7): at mount and then every 60 seconds after the previous
 * check, `HEAD /`; the first `ETag` seen is this page's version. When a later one differs and
 * `mayReload` is true at that moment (the latest state poll succeeded, no token form is shown),
 * `reload` is called, once per page life, so a server whose `ETag` changes on every answer
 * cannot make the wall reload again and again. A failed check is ignored.
 */
export function useNewVersion(reload: () => void, mayReload: boolean): void {
  const may = useRef(mayReload);
  may.current = mayReload;
  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  useEffect(() => {
    let first: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let limit: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;

    const seen = (tag: string) => {
      if (first === undefined) {
        first = tag;
      } else if (tag !== first && may.current && !reloadAsked) {
        reloadAsked = true;
        reloadRef.current();
      }
    };

    function check() {
      const mine = new AbortController();
      controller = mine;
      limit = setTimeout(() => mine.abort(), READ_LIMIT_MS);
      fetch('/', { method: 'HEAD', cache: 'no-store', signal: mine.signal })
        .then(
          (response) => {
            const tag = response.headers?.get('ETag') ?? null;
            if (!mine.signal.aborted && response.ok && tag !== null) seen(tag);
          },
          () => {
            // The server may be away or restarting: the next check tries again.
          },
        )
        .finally(() => {
          clearTimeout(limit);
          if (!stopped) timer = setTimeout(check, VERSION_MS);
        });
    }

    check();
    return () => {
      stopped = true;
      clearTimeout(timer);
      clearTimeout(limit);
      controller?.abort();
    };
  }, []);
}
