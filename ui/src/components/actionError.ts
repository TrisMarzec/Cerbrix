import { ApiError } from '../api/client';

/** The sentence a dialog shows when its request failed. */
export function describeActionError(e: unknown): string {
  if (!(e instanceof ApiError)) return 'Could not reach the server.';
  switch (e.status) {
    case 0:
      return 'Could not reach the server.';
    case 403:
      return 'Your role may not do this.';
    case 409:
      return 'This problem is already resolved.';
    default:
      return e.message;
  }
}
