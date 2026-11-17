import { vi } from 'vitest';

/** One request the code under test made, with the body already parsed. */
export interface RecordedRequest {
  method: string;
  /** The path without the `/api/v1` prefix and without the query, as sent (still percent-encoded). */
  path: string;
  query: URLSearchParams;
  headers: Headers;
  /** The parsed JSON body; `undefined` when the request had none. */
  body: unknown;
  init: RequestInit;
}

/** An answer: a status with a JSON or text body, or `'network'` for a fetch that fails. */
export type Answer = { status?: number; json?: unknown; text?: string } | 'network';
export type Responder = Answer | ((request: RecordedRequest) => Answer | Promise<Answer>);

export interface FetchMock {
  requests: RecordedRequest[];
  /** Number of requests recorded for a key such as `GET /status`. */
  count(key: string): number;
  /** Sets or replaces the answer for a key. */
  on(key: string, responder: Responder | Responder[]): void;
  restore(): void;
}

function isAborted(init: RequestInit): boolean {
  return init.signal?.aborted === true;
}

/**
 * The part of a Response the client uses. Built by hand because the body of a real Response is
 * read through streams that fake timers cannot flush; this one settles in microtasks.
 */
function responseFor(answer: Exclude<Answer, 'network'>): Response {
  const status = answer.status ?? 200;
  const text = status === 204 ? '' : (answer.text ?? JSON.stringify(answer.json ?? {}));
  const fake = {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve().then(() => JSON.parse(text) as unknown),
  };
  return fake as unknown as Response;
}

const PREFIX = '/api/v1';

/**
 * Replaces the global `fetch`. `routes` maps "METHOD /path" (the path without `/api/v1`, query ignored)
 * to an answer, a function that computes one, or a list of answers used in turn (the last one repeats).
 * A request nobody answers gets `404 not_found`. Every request is recorded in `requests`.
 * Call `restore()` (for example in `afterEach`) to put the real `fetch` back.
 */
export function mockFetch(routes: Record<string, Responder | Responder[]> = {}): FetchMock {
  const table = new Map<string, Responder[]>();
  const set = (key: string, responder: Responder | Responder[]) => {
    table.set(key, Array.isArray(responder) ? [...responder] : [responder]);
  };
  for (const [key, responder] of Object.entries(routes)) set(key, responder);

  const requests: RecordedRequest[] = [];
  const keyOf = (r: RecordedRequest) => `${r.method} ${r.path}`;

  const fake = async (input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> => {
    const url = new URL(String(input), 'http://localhost');
    const request: RecordedRequest = {
      method: (init.method ?? 'GET').toUpperCase(),
      path: url.pathname.startsWith(PREFIX) ? url.pathname.slice(PREFIX.length) : url.pathname,
      query: url.searchParams,
      headers: new Headers(init.headers),
      body: typeof init.body === 'string' ? (JSON.parse(init.body) as unknown) : undefined,
      init,
    };
    requests.push(request);
    if (isAborted(init)) throw new DOMException('Aborted', 'AbortError');

    const list = table.get(keyOf(request));
    let answer: Answer = {
      status: 404,
      json: { error: 'not_found', message: `no mock for ${keyOf(request)}` },
    };
    if (list !== undefined && list.length > 0) {
      const responder = list.length > 1 ? list.shift() : list[0];
      if (responder !== undefined) {
        answer = typeof responder === 'function' ? await responder(request) : responder;
      }
    }
    if (answer === 'network') throw new TypeError('Failed to fetch');
    if (isAborted(init)) throw new DOMException('Aborted', 'AbortError');
    return responseFor(answer);
  };

  vi.stubGlobal('fetch', fake);
  return {
    requests,
    count: (key) => requests.filter((r) => keyOf(r) === key).length,
    on: set,
    restore: () => {
      vi.unstubAllGlobals();
    },
  };
}
