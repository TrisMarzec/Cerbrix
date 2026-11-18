import { ApiError } from '../api/client';

/** Sentences a call site gives for statuses it can receive, by status. */
export type ErrorTexts = Partial<Record<number, string>>;

/**
 * The sentence shown for a failed request: the caller's sentence for the status first, then
 * "Could not reach the server." for a network failure (status 0, or anything that is not an
 * `ApiError`), "Your role may not do this." for 403, "The server is busy, try again in a moment."
 * for 429, and the server's message for anything else. A network failure has no override: it is
 * always the same sentence.
 */
export function describeError(e: unknown, overrides: ErrorTexts = {}): string {
  if (!(e instanceof ApiError) || e.status === 0) return 'Could not reach the server.';
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
