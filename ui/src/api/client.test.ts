import { api, ApiError, newSessionGeneration, onSessionDoubt, onUnauthorized, seg } from './client';
import { mockFetch, type FetchMock } from '../test/mockFetch';

let mock: FetchMock;
afterEach(() => {
  mock.restore();
  onUnauthorized(() => {});
  onSessionDoubt(() => {});
  vi.useRealTimers();
});

describe('api.get', () => {
  it('builds the query, drops undefined values and sends the cookie rules', async () => {
    mock = mockFetch({ 'GET /problems': { json: { items: [], total: 0 } } });
    await api.get('/problems', { state: 'open', limit: 50, offset: undefined });
    const request = mock.requests[0];
    expect(request?.query.get('state')).toBe('open');
    expect(request?.query.get('limit')).toBe('50');
    expect(request?.query.has('offset')).toBe(false);
    expect(request?.init.credentials).toBe('same-origin');
    expect(request?.init.cache).toBe('no-store');
  });

  it('turns an error answer into an ApiError with the code and message of the body', async () => {
    mock = mockFetch({
      'GET /hosts': { status: 409, json: { error: 'conflict', message: 'already there' } },
    });
    const error: unknown = await api.get('/hosts').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 409, code: 'conflict', message: 'already there' });
  });

  it('gives an error body that is not JSON the code internal', async () => {
    mock = mockFetch({ 'GET /hosts': { status: 502, text: '<html>Bad gateway</html>' } });
    const error: unknown = await api.get('/hosts').catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 502, code: 'internal' });
  });

  it('gives a failing fetch the code network and status 0', async () => {
    mock = mockFetch({ 'GET /hosts': 'network' });
    const error: unknown = await api.get('/hosts').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 0, code: 'network' });
  });
});

describe('api.send', () => {
  it('sends JSON, and {} for a POST without a body', async () => {
    mock = mockFetch({ 'POST /thing': { json: { ok: true } } });
    await api.send('POST', '/thing');
    await api.send('POST', '/thing', { comment: 'x' });
    expect(mock.requests[0]?.headers.get('Content-Type')).toBe('application/json');
    expect(mock.requests[0]?.init.body).toBe('{}');
    expect(mock.requests[1]?.body).toEqual({ comment: 'x' });
  });

  it('sends nothing for a DELETE without a body', async () => {
    mock = mockFetch({ 'DELETE /thing': { status: 204 } });
    await api.send('DELETE', '/thing');
    expect(mock.requests[0]?.init.body).toBeUndefined();
    expect(mock.requests[0]?.headers.has('Content-Type')).toBe(false);
  });

  it('answers undefined for 204', async () => {
    mock = mockFetch({ 'DELETE /thing': { status: 204 } });
    await expect(api.send('DELETE', '/thing')).resolves.toBeUndefined();
  });
});

describe('401', () => {
  it('calls the handler once and still rejects', async () => {
    mock = mockFetch({
      'GET /status': { status: 401, json: { error: 'unauthorized', message: 'no session' } },
    });
    const handler = vi.fn();
    onUnauthorized(handler);
    const error: unknown = await api.get('/status').catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 401, code: 'unauthorized' });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('is an ordinary error for the login request', async () => {
    mock = mockFetch({
      'POST /session': { status: 401, json: { error: 'unauthorized', message: 'wrong' } },
    });
    const handler = vi.fn();
    onUnauthorized(handler);
    await expect(api.send('POST', '/session', { name: 'a', password: 'b' })).rejects.toMatchObject({
      status: 401,
    });
    expect(handler).not.toHaveBeenCalled();
  });
});

describe('seg', () => {
  it('encodes a path segment', () => {
    expect(seg('a/b c?')).toBe('a%2Fb%20c%3F');
  });

  it('refuses the dot segments, which a browser resolves even when encoded', () => {
    expect(() => seg('.')).toThrow();
    expect(() => seg('..')).toThrow();
    expect(seg('a.b')).toBe('a.b');
    expect(seg('...')).toBe('...');
  });
});

describe('session generation', () => {
  it('does not report a 401 of a request that started in an earlier generation', async () => {
    let answer: (a: { status: number; json: unknown }) => void = () => {};
    mock = mockFetch({
      'GET /slow': () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const handler = vi.fn();
    onUnauthorized(handler);
    const pending = api.get('/slow').catch((e: unknown) => e);
    await vi.waitFor(() => {
      expect(mock.requests).toHaveLength(1);
    });
    newSessionGeneration();
    answer({ status: 401, json: { error: 'unauthorized', message: 'old' } });
    await expect(pending).resolves.toMatchObject({ status: 401 });
    expect(handler).not.toHaveBeenCalled();
  });

  it('does not report a 401 of probe', async () => {
    mock = mockFetch({ 'GET /session': { status: 401, json: { error: 'unauthorized', message: 'x' } } });
    const handler = vi.fn();
    onUnauthorized(handler);
    await expect(api.probe('/session')).rejects.toMatchObject({ status: 401 });
    expect(handler).not.toHaveBeenCalled();
  });
});

describe('403', () => {
  it('reports a 403 as a doubt about the session and still rejects', async () => {
    mock = mockFetch({ 'GET /users': { status: 403, json: { error: 'forbidden', message: 'no' } } });
    const doubt = vi.fn();
    const unauthorized = vi.fn();
    onSessionDoubt(doubt);
    onUnauthorized(unauthorized);
    await expect(api.get('/users')).rejects.toMatchObject({ status: 403 });
    expect(doubt).toHaveBeenCalledTimes(1);
    expect(unauthorized).not.toHaveBeenCalled();
  });
});

describe('time limit', () => {
  const hang = () => new Promise<never>(() => {});

  it('gives up a GET after 15 s with the code network and aborts the request', async () => {
    vi.useFakeTimers();
    mock = mockFetch({ 'GET /status': hang });
    let failure: unknown;
    void api.get('/status').catch((e: unknown) => {
      failure = e;
    });
    await vi.advanceTimersByTimeAsync(14_999);
    expect(failure).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).toMatchObject({ status: 0, code: 'network' });
    expect(mock.requests[0]?.init.signal?.aborted).toBe(true);
  });

  it('gives up a probe after 15 s', async () => {
    vi.useFakeTimers();
    mock = mockFetch({ 'GET /session': hang });
    let failure: unknown;
    void api.probe('/session').catch((e: unknown) => {
      failure = e;
    });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(failure).toMatchObject({ status: 0, code: 'network' });
  });

  it('gives a send 30 s', async () => {
    vi.useFakeTimers();
    mock = mockFetch({ 'POST /thing': hang });
    let failure: unknown;
    void api.send('POST', '/thing').catch((e: unknown) => {
      failure = e;
    });
    await vi.advanceTimersByTimeAsync(29_999);
    expect(failure).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    expect(failure).toMatchObject({ status: 0, code: 'network' });
  });

  it('also limits an answer whose body never arrives', async () => {
    vi.useFakeTimers();
    mock = mockFetch();
    vi.stubGlobal('fetch', () =>
      Promise.resolve({ ok: true, status: 200, headers: new Headers(), json: hang } as unknown as Response),
    );
    let failure: unknown;
    void api.get('/status').catch((e: unknown) => {
      failure = e;
    });
    await vi.advanceTimersByTimeAsync(15_000);
    expect(failure).toMatchObject({ status: 0, code: 'network' });
  });

  it("still follows the caller's abort signal", async () => {
    mock = mockFetch({ 'GET /status': hang });
    const controller = new AbortController();
    const result = api.get('/status', undefined, controller.signal).catch((e: unknown) => e);
    await vi.waitFor(() => {
      expect(mock.requests).toHaveLength(1);
    });
    controller.abort();
    await expect(result).resolves.toMatchObject({ status: 0, code: 'network' });
    expect(mock.requests[0]?.init.signal?.aborted).toBe(true);
  });
});

describe('server date', () => {
  it('gives the Date header of the answer with the data', async () => {
    mock = mockFetch({
      'GET /status': { json: { ok: 1 }, headers: { Date: 'Tue, 17 Nov 2026 13:30:00 GMT' } },
    });
    const answer = await api.getDated<{ ok: number }>('/status');
    expect(answer.data).toEqual({ ok: 1 });
    expect(answer.serverDate?.toISOString()).toBe('2026-11-17T13:30:00.000Z');
  });

  it('gives no date when the header is missing or not a date', async () => {
    mock = mockFetch({ 'GET /status': [{ json: {} }, { json: {}, headers: { Date: 'soon' } }] });
    expect((await api.getDated('/status')).serverDate).toBeUndefined();
    expect((await api.getDated('/status')).serverDate).toBeUndefined();
  });
});
