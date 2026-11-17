const BASE = '/api/v1';

/** An answer the server gave with an error status, or `code: "network"` with status 0 when fetch itself failed. */
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

let generation = 0;

/**
 * Starts a new session generation (a login or a logout). A `401` that belongs to a request started
 * in an earlier generation says nothing about the current session and is not reported.
 */
export function newSessionGeneration(): number {
  generation += 1;
  return generation;
}

export function currentSessionGeneration(): number {
  return generation;
}

/** Encodes one path segment (a host name, an id); dot segments are encoded too so a path cannot climb. */
export function seg(value: string): string {
  if (value === '.') return '%2E';
  if (value === '..') return '%2E%2E';
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

async function request<T>(
  method: string,
  path: string,
  init: RequestInit,
  signal?: AbortSignal,
  notify = true,
): Promise<T> {
  const started = generation;
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
    throw new ApiError(0, 'network', 'Could not reach the server');
  }
  if (!response.ok) {
    // The login request answers 401 for a wrong password: that is not a lost session.
    const isLogin = method === 'POST' && path === `${BASE}/session`;
    if (response.status === 401 && notify && !isLogin && signal?.aborted !== true && started === generation) {
      unauthorized();
    }
    throw await errorFrom(response);
  }
  if (response.status === 204) return undefined as T;
  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiError(response.status, 'internal', 'The server sent an answer that is not JSON');
  }
}

export const api = {
  get<T>(path: string, query?: Query, signal?: AbortSignal): Promise<T> {
    return request<T>('GET', withQuery(path, query), {}, signal);
  },

  /** Like `get`, but a 401 is not reported to the `onUnauthorized` handler (the session check itself). */
  probe<T>(path: string): Promise<T> {
    return request<T>('GET', BASE + path, {}, undefined, false);
  },

  send<T>(method: 'POST' | 'PUT' | 'PATCH' | 'DELETE', path: string, body?: unknown): Promise<T> {
    if (body === undefined && method === 'DELETE') {
      return request<T>(method, BASE + path, {});
    }
    return request<T>(method, BASE + path, {
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body ?? {}),
    });
  },
};
