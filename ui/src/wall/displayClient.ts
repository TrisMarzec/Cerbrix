import { ApiError, dateOf, errorFrom, networkError, READ_LIMIT_MS, recordServerDate, seg } from '../api/client';

/*
 * The wall monitor's side of the API: the two display routes, with the display token. Ruling R1:
 * this is the only module of the UI that uses browser storage, and only for that token. The token
 * goes nowhere but into the `Authorization` header of these requests: not into a URL, a log line
 * or an error message.
 */

function key(displayId: string): string {
  return `cerbrix.display.${displayId}`;
}

/** Tokens the browser storage refused (private mode, storage switched off): kept for the page's life. */
const unkept = new Map<string, string>();

/** The browser storage; undefined where even asking for it throws. */
function storage(): Storage | undefined {
  try {
    return window.localStorage;
  } catch {
    return undefined;
  }
}

/** The token of a display, as the browser keeps it; undefined without one. */
export function storedToken(displayId: string): string | undefined {
  const kept = unkept.get(displayId);
  if (kept !== undefined) return kept;
  try {
    return storage()?.getItem(key(displayId)) ?? undefined;
  } catch {
    return undefined;
  }
}

/**
 * Keeps the token of a display in the browser storage; when the storage refuses it, in this page
 * only (`tokenKept` then says false).
 */
export function storeToken(displayId: string, token: string): void {
  try {
    const store = storage();
    if (store === undefined) throw new Error('no storage');
    store.setItem(key(displayId), token);
    unkept.delete(displayId);
  } catch {
    unkept.set(displayId, token);
  }
}

/** Forgets the token of a display, in the storage and in this page. */
export function forgetToken(displayId: string): void {
  unkept.delete(displayId);
  try {
    storage()?.removeItem(key(displayId));
  } catch {
    // Nothing was kept there.
  }
}

/** False when the display's token lives in this page only and is gone after a restart. */
export function tokenKept(displayId: string): boolean {
  return !unkept.has(displayId);
}

async function exchange<T>(url: string, token: string | undefined, signal: AbortSignal): Promise<T> {
  let response: Response;
  try {
    response = await fetch(url, {
      method: 'GET',
      // A logged-in user's session cookie answers too, so a desk user needs no token.
      credentials: 'same-origin',
      cache: 'no-store',
      // A redirect must never carry the bearer token to another path: it fails as no answer.
      redirect: 'error',
      headers: token === undefined ? {} : { Authorization: `Bearer ${token}` },
      signal,
    });
  } catch {
    throw networkError();
  }
  recordServerDate(dateOf(response));
  if (!response.ok) throw await errorFrom(response);
  try {
    return (await response.json()) as T;
  } catch {
    if (signal.aborted) throw networkError();
    throw new ApiError(response.status, 'internal', 'The server sent an answer that is not JSON');
  }
}

/**
 * `GET /api/v1/display/<id><path>`: `''` for the display's state, `/dashboards/<id>` (built with
 * `seg`) for a dashboard. Sends `token`, by default the stored one, as a bearer token, and nothing
 * when there is none. As the desk client: a 15-second limit, errors as `ApiError` (`network` with
 * status 0 when no answer came), the server's clock recorded from the `Date` header. Unlike it,
 * nothing here touches the session: a 401 is the caller's to handle.
 */
export function displayGet<T>(
  displayId: string,
  path: string,
  signal?: AbortSignal,
  token: string | undefined = storedToken(displayId),
): Promise<T> {
  let url: string;
  try {
    url = `/api/v1/display/${seg(displayId)}${path}`;
  } catch {
    // `.` and `..` cannot be sent as a path segment, and no display has such an id.
    return Promise.reject(new ApiError(404, 'not_found', 'no such display'));
  }
  const controller = new AbortController();
  return new Promise<T>((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', stop);
    };
    function stop() {
      finish();
      controller.abort();
      reject(networkError());
    }
    timer = setTimeout(stop, READ_LIMIT_MS);
    if (signal?.aborted === true) {
      stop();
      return;
    }
    signal?.addEventListener('abort', stop);
    exchange<T>(url, token, controller.signal).then(
      (answer) => {
        finish();
        resolve(answer);
      },
      (e: unknown) => {
        finish();
        reject(e instanceof ApiError ? e : networkError());
      },
    );
  });
}
