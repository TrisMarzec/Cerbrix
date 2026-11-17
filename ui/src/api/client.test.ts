import { api, ApiError, onUnauthorized, seg } from './client';
import { mockFetch, type FetchMock } from '../test/mockFetch';

let mock: FetchMock;
afterEach(() => {
  mock.restore();
  onUnauthorized(() => {});
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
});
