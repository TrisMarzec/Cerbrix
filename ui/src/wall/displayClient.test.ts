import { vi } from 'vitest';

import { ApiError, READ_LIMIT_MS, resetServerSkew, serverSkewMs } from '../api/client';
import { mockFetch, type FetchMock } from '../test/mockFetch';
import { displayGet, forgetToken, storedToken, storeToken, tokenKept } from './displayClient';

const TOKEN = 'cbx_dt_secret-token-value';
const STATE = 'GET /display/noc-left';

let mock: FetchMock | undefined;

afterEach(() => {
  mock?.restore();
  mock = undefined;
  vi.restoreAllMocks();
  vi.useRealTimers();
  forgetToken('noc-left');
  forgetToken('other');
  window.localStorage.clear();
  resetServerSkew();
});

function caught(promise: Promise<unknown>): Promise<ApiError> {
  return promise.then(
    () => {
      throw new Error('expected an error');
    },
    (e: unknown) => {
      expect(e).toBeInstanceOf(ApiError);
      return e as ApiError;
    },
  );
}

describe('the token storage', () => {
  it('keeps one token per display in the browser storage, under the display key', () => {
    expect(storedToken('noc-left')).toBeUndefined();
    storeToken('noc-left', TOKEN);
    expect(storedToken('noc-left')).toBe(TOKEN);
    expect(window.localStorage.getItem('cerbrix.display.noc-left')).toBe(TOKEN);
    expect(storedToken('other')).toBeUndefined();
    expect(tokenKept('noc-left')).toBe(true);
    forgetToken('noc-left');
    expect(storedToken('noc-left')).toBeUndefined();
    expect(window.localStorage.getItem('cerbrix.display.noc-left')).toBeNull();
  });

  it('keeps the token for the life of the page when the storage refuses it, and says so', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('The quota has been exceeded.', 'QuotaExceededError');
    });
    storeToken('noc-left', TOKEN);
    expect(storedToken('noc-left')).toBe(TOKEN);
    expect(tokenKept('noc-left')).toBe(false);
    forgetToken('noc-left');
    expect(storedToken('noc-left')).toBeUndefined();
    expect(tokenKept('noc-left')).toBe(true);
  });

  it('works without any storage at all (reading it throws, as in a locked-down browser)', () => {
    vi.spyOn(window, 'localStorage', 'get').mockImplementation(() => {
      throw new DOMException('Access is denied.', 'SecurityError');
    });
    expect(storedToken('noc-left')).toBeUndefined();
    storeToken('noc-left', TOKEN);
    expect(storedToken('noc-left')).toBe(TOKEN);
    expect(tokenKept('noc-left')).toBe(false);
    expect(() => forgetToken('noc-left')).not.toThrow();
    expect(storedToken('noc-left')).toBeUndefined();
  });
});

describe('displayGet', () => {
  it('asks without a token when none is stored, sending the session cookie', async () => {
    mock = mockFetch({ [STATE]: { json: { id: 'noc-left' } } });
    await expect(displayGet('noc-left', '')).resolves.toEqual({ id: 'noc-left' });
    const [request] = mock.requests;
    expect(request?.headers.has('Authorization')).toBe(false);
    expect(request?.init.credentials).toBe('same-origin');
    expect(request?.init.cache).toBe('no-store');
  });

  it('sends the stored token as a bearer token, and nowhere in the address', async () => {
    storeToken('noc-left', TOKEN);
    mock = mockFetch({ 'GET /display/noc-left/dashboards/kunde%20a': { json: {} } });
    await displayGet('noc-left', '/dashboards/kunde%20a');
    const [request] = mock.requests;
    expect(request?.headers.get('Authorization')).toBe(`Bearer ${TOKEN}`);
    expect(request?.path).toBe('/display/noc-left/dashboards/kunde%20a');
    expect(request?.query.toString()).toBe('');
    // With a token, never the session cookie too (R16): the server would answer as the user.
    expect(request?.init.credentials).toBe('omit');
  });

  it('sends a token given for one request (the token form checks it) instead of the stored one', async () => {
    storeToken('noc-left', 'cbx_dt_old');
    mock = mockFetch({ [STATE]: { json: {} } });
    await displayGet('noc-left', '', undefined, TOKEN);
    expect(mock.requests[0]?.headers.get('Authorization')).toBe(`Bearer ${TOKEN}`);
    expect(mock.requests[0]?.init.credentials).toBe('omit');
  });

  it('encodes the display id as a path segment', async () => {
    mock = mockFetch({ 'GET /display/a%2Fb': { json: {} } });
    await displayGet('a/b', '');
    expect(mock.requests[0]?.path).toBe('/display/a%2Fb');
  });

  it('answers 404 for a display id that cannot be a path segment, without asking', async () => {
    mock = mockFetch({});
    const error = await caught(displayGet('..', ''));
    expect(error.status).toBe(404);
    expect(mock.requests).toHaveLength(0);
  });

  it('gives the error of the server as ApiError, without the token in its message', async () => {
    storeToken('noc-left', TOKEN);
    mock = mockFetch({ [STATE]: { status: 401, json: { error: 'unauthorized', message: 'bad token' } } });
    const error = await caught(displayGet('noc-left', ''));
    expect(error).toMatchObject({ status: 401, code: 'unauthorized', message: 'bad token' });
    expect(error.message).not.toContain(TOKEN);
  });

  it('gives a server error that is not JSON with the status', async () => {
    mock = mockFetch({ [STATE]: { status: 502, text: '<html>Bad gateway</html>' } });
    expect(await caught(displayGet('noc-left', ''))).toMatchObject({ status: 502, code: 'internal' });
  });

  it('refuses to follow a redirect, which fails as no answer (a network error)', async () => {
    storeToken('noc-left', TOKEN);
    // A fetch with `redirect: 'error'` rejects with a TypeError when the server redirects.
    mock = mockFetch({
      [STATE]: (request) => (request.init.redirect === 'error' ? 'network' : { json: { followed: true } }),
    });
    expect(await caught(displayGet('noc-left', ''))).toMatchObject({ status: 0, code: 'network' });
    expect(mock.requests[0]?.init.redirect).toBe('error');
  });

  it('does not take the clock of an answer that is not the API (an HTML 502 of a gateway)', async () => {
    const ahead = new Date(Date.now() + 5 * 60_000).toUTCString();
    mock = mockFetch({
      [STATE]: { status: 502, text: '<html>Bad gateway</html>', headers: { 'Content-Type': 'text/html', Date: ahead } },
    });
    expect(await caught(displayGet('noc-left', ''))).toMatchObject({ status: 502 });
    expect(serverSkewMs()).toBe(0);
  });

  it('gives a failed fetch as a network error with status 0', async () => {
    mock = mockFetch({ [STATE]: 'network' });
    expect(await caught(displayGet('noc-left', ''))).toMatchObject({ status: 0, code: 'network' });
  });

  it('gives up after 15 seconds with a network error and aborts the request', async () => {
    vi.useFakeTimers();
    mock = mockFetch({ [STATE]: () => new Promise(() => {}) });
    const result = caught(displayGet('noc-left', ''));
    await vi.advanceTimersByTimeAsync(READ_LIMIT_MS - 1);
    expect(mock.requests[0]?.init.signal?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await result).toMatchObject({ status: 0, code: 'network' });
    expect(mock.requests[0]?.init.signal?.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("ends at once with a network error when the caller's signal aborts", async () => {
    mock = mockFetch({ [STATE]: () => new Promise(() => {}) });
    const controller = new AbortController();
    const result = caught(displayGet('noc-left', '', controller.signal));
    controller.abort();
    expect(await result).toMatchObject({ status: 0, code: 'network' });
  });

  it("records the server's clock from the Date header, also of an error answer", async () => {
    const ahead = new Date(Date.now() + 5 * 60_000).toUTCString();
    mock = mockFetch({ [STATE]: { status: 401, json: {}, headers: { Date: ahead } } });
    await caught(displayGet('noc-left', ''));
    expect(serverSkewMs()).toBeGreaterThan(4 * 60_000);
  });

  it('never writes to the console', async () => {
    const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m));
    storeToken('noc-left', TOKEN);
    mock = mockFetch({ [STATE]: 'network' });
    await caught(displayGet('noc-left', ''));
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
  });
});
