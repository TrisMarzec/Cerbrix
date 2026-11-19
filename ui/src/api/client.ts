const BASE = '/api/v1';

/** How long a read (`get`, `probe`) may take before it counts as a network failure. */
export const READ_LIMIT_MS = 15_000;
/** How long a change (`send`) may take before it counts as a network failure. */
export const SEND_LIMIT_MS = 30_000;

/**
 * An answer the server gave with an error status, or `code: "network"` with status 0 when fetch
 * itself failed, the request was aborted, or no answer arrived within the time limit.
 */
export class ApiError extends Error {
  status: number;
  code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

let unauthorized: () => void = () => {};

/** Registers the one handler for a `401` on any request except the login itself. */
export function onUnauthorized(handler: () => void): void {
  unauthorized = handler;
}

let doubt: () => void = () => {};

/**
 * Registers the one handler for a `403` on any request: the session may now belong to another user
 * (someone logged in in another tab), so the caller asks again who is logged in.
 */
export function onSessionDoubt(handler: () => void): void {
  doubt = handler;
}

let generation = 0;

/**
 * Starts a new session generation (a login, a logout, another user behind the cookie). A `401` or
 * `403` that belongs to a request started in an earlier generation says nothing about the current
 * session and is not reported.
 */
export function newSessionGeneration(): number {
  generation += 1;
  return generation;
}

export function currentSessionGeneration(): number {
  return generation;
}

/**
 * Encodes one path segment (a host name, an id). `.` and `..` are refused: a browser resolves them
 * as dot segments even when they are percent-encoded, and they are never a valid name or id here.
 */
export function seg(value: string): string {
  if (value === '.' || value === '..') throw new Error(`"${value}" is not a valid path segment`);
  return encodeURIComponent(value);
}

type Query = Record<string, string | number | undefined>;

function withQuery(path: string, query: Query | undefined): string {
  if (query === undefined) return BASE + path;
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) params.set(key, String(value));
  }
  const text = params.toString();
  return text === '' ? BASE + path : `${BASE}${path}?${text}`;
}

async function errorFrom(response: Response): Promise<ApiError> {
  let code = 'internal';
  let message = `The server answered ${response.status}`;
  try {
    const body: unknown = await response.json();
    if (typeof body === 'object' && body !== null) {
      const fields = body as { error?: unknown; message?: unknown };
      if (typeof fields.error === 'string') code = fields.error;
      if (typeof fields.message === 'string') message = fields.message;
    }
  } catch {
    // The body was not JSON: keep the defaults.
  }
  return new ApiError(response.status, code, message);
}

function networkError(): ApiError {
  return new ApiError(0, 'network', 'Could not reach the server');
}

function dateOf(response: Response): Date | undefined {
  const text = response.headers?.get('Date') ?? null;
  if (text === null) return undefined;
  const date = new Date(text);
  return Number.isNaN(date.getTime()) ? undefined : date;
}

/**
 * The `Date` header has a resolution of one second: a difference that moves by this much or less
 * is the same clock, and keeping it keeps the server-corrected time from wobbling on every answer.
 */
const SKEW_TOLERANCE_MS = 2000;

let skewMs = 0;

/**
 * Records the server's clock from the `Date` header of an answer: how far it is ahead of the
 * browser's. Every answer through `api` records it; code that fetches by itself calls this. The
 * recorded difference changes only when the new one differs from it by more than two seconds.
 */
export function recordServerDate(date: Date | undefined): void {
  if (date === undefined) return;
  const next = date.getTime() - Date.now();
  if (Math.abs(next - skewMs) > SKEW_TOLERANCE_MS) skewMs = next;
}

/** How far the server's clock is ahead of the browser's, in ms, from the dated answers; 0 before one. */
export function serverSkewMs(): number {
  return skewMs;
}

/** Forgets the recorded difference, as before the first answer; for tests. */
export function resetServerSkew(): void {
  skewMs = 0;
}

interface Options {
  signal?: AbortSignal | undefined;
  /** Whether a 401 or 403 is reported to the handlers (not for the session check itself). */
  notify: boolean;
  limitMs: number;
}

async function exchange<T>(
  method: string,
  path: string,
  init: RequestInit,
  signal: AbortSignal,
  options: Options,
  started: number,
): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      method,
      credentials: 'same-origin',
      cache: 'no-store',
      signal,
    });
  } catch {
    throw networkError();
  }
  recordServerDate(dateOf(response));
  if (!response.ok) {
    const current = started === generation && options.signal?.aborted !== true;
    // The login request answers 401 for a wrong password: that is not a lost session.
    const isLogin = method === 'POST' && path === `${BASE}/session`;
    if (options.notify && current) {
      if (response.status === 401 && !isLogin) unauthorized();
      if (response.status === 403) doubt();
    }
    throw await errorFrom(response);
  }
  if (response.status === 204) return undefined as T;
  try {
    return (await response.json()) as T;
  } catch {
    if (signal.aborted) throw networkError();
    throw new ApiError(response.status, 'internal', 'The server sent an answer that is not JSON');
  }
}

/**
 * One request with a time limit: the caller's signal and the limit both abort it, and either ends
 * the promise at once with a network error, also when fetch or the body does not react to the abort.
 */
function request<T>(method: string, path: string, init: RequestInit, options: Options): Promise<T> {
  const started = generation;
  const controller = new AbortController();
  return new Promise<T>((resolve, reject) => {
    const stop = () => {
      controller.abort();
      reject(networkError());
    };
    const timer = setTimeout(stop, options.limitMs);
    const caller = options.signal;
    if (caller?.aborted === true) stop();
    caller?.addEventListener('abort', stop);
    const finish = () => {
      clearTimeout(timer);
      caller?.removeEventListener('abort', stop);
    };
    exchange<T>(method, path, init, controller.signal, options, started).then(
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

export const api = {
  get<T>(path: string, query?: Query, signal?: AbortSignal): Promise<T> {
    return request<T>('GET', withQuery(path, query), {}, { signal, notify: true, limitMs: READ_LIMIT_MS });
  },

  /** Like `get`, but a 401 or 403 is not reported to the handlers (the session check itself). */
  probe<T>(path: string): Promise<T> {
    return request<T>('GET', BASE + path, {}, { notify: false, limitMs: READ_LIMIT_MS });
  },

  send<T>(method: 'POST' | 'PUT' | 'PATCH' | 'DELETE', path: string, body?: unknown): Promise<T> {
    const options: Options = { notify: true, limitMs: SEND_LIMIT_MS };
    const init: RequestInit =
      body === undefined && method === 'DELETE'
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body ?? {}) };
    return request<T>(method, BASE + path, init, options);
  },
};
