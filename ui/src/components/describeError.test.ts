import { ApiError } from '../api/client';
import { describeError } from './describeError';

describe('describeError', () => {
  it('says the server could not be reached for status 0 and for anything that is not an ApiError', () => {
    expect(describeError(new ApiError(0, 'network', 'x'))).toBe('Could not reach the server.');
    expect(describeError(new Error('boom'))).toBe('Could not reach the server.');
    expect(describeError(undefined)).toBe('Could not reach the server.');
  });

  it('has a sentence for 403 and 429', () => {
    expect(describeError(new ApiError(403, 'forbidden', 'admin only'))).toBe('Your role may not do this.');
    expect(describeError(new ApiError(429, 'busy', 'slow down'))).toBe(
      'The server is busy, try again in a moment.',
    );
  });

  it("gives the server's message for anything else", () => {
    expect(describeError(new ApiError(409, 'conflict', 'This name is in use.'))).toBe('This name is in use.');
    expect(describeError(new ApiError(500, 'internal', 'The database is down.'))).toBe(
      'The database is down.',
    );
  });

  it('has a sentence for a server without its database and for a server that took too long', () => {
    const down = new ApiError(503, 'database_unavailable', 'the database is not reachable');
    const slow = new ApiError(503, 'timeout', 'the request took too long');
    expect(describeError(down)).toBe('The server cannot reach its database.');
    expect(describeError(slow)).toBe('The server took too long to answer.');
    // These say more than a caller's sentence for 503, which is about something else.
    expect(describeError(down, { 503: 'The metrics store did not answer.' })).toBe(
      'The server cannot reach its database.',
    );
    expect(describeError(slow, { 503: 'The metrics store did not answer.' })).toBe(
      'The server took too long to answer.',
    );
  });

  it('does not take a code for a sentence because an object has a property of that name', () => {
    expect(describeError(new ApiError(500, 'constructor', 'the server message'))).toBe('the server message');
  });

  it('takes a sentence per status from the caller first', () => {
    const overrides = { 409: 'This problem is already resolved.', 403: 'The current password is wrong.' };
    expect(describeError(new ApiError(409, 'conflict', 'x'), overrides)).toBe('This problem is already resolved.');
    expect(describeError(new ApiError(403, 'forbidden', 'x'), overrides)).toBe('The current password is wrong.');
    expect(describeError(new ApiError(404, 'not_found', 'gone'), overrides)).toBe('gone');
    expect(describeError(new ApiError(0, 'network', 'x'), { 0: 'never' })).toBe('Could not reach the server.');
  });
});
