import { ApiError } from '../api/client';

/** Sentences a call site gives for statuses it can receive, by status. */
export type ErrorTexts = Partial<Record<number, string>>;

/** Sentences for error codes that mean the same wherever they come from; no caller overrides them. */
const BY_CODE: Partial<Record<string, string>> = {
  database_unavailable: 'The server cannot reach its database.',
  timeout: 'The server took too long to answer.',
};

/**
 * The sentence shown for a failed request: "Could not reach the server." for a network failure
 * (status 0, or anything that is not an `ApiError`), the sentence for a database that cannot be
 * reached (`database_unavailable`) or a request that took too long (`timeout`), then the caller's
 * sentence for the status, "Your role may not do this." for 403, "The server is busy, try again
 * in a moment." for 429, and the server's message for anything else. The first three have no
 * override: they are always the same sentence.
 */
export function describeError(e: unknown, overrides: ErrorTexts = {}): string {
  if (!(e instanceof ApiError) || e.status === 0) return 'Could not reach the server.';
  const general = BY_CODE[e.code];
  if (general !== undefined) return general;
  const own = overrides[e.status];
  if (own !== undefined) return own;
  switch (e.status) {
    case 403:
      return 'Your role may not do this.';
    case 429:
      return 'The server is busy, try again in a moment.';
    default:
      return e.message;
  }
}

/** True for an `ApiError` with this status. */
export function hasStatus(e: unknown, status: number): boolean {
  return e instanceof ApiError && e.status === status;
}
