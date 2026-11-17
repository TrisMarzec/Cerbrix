import { useCallback } from 'preact/hooks';

import { navigate, useRoute } from './router';

type Changes = Record<string, string | undefined>;

/**
 * The query of the address bar and a setter for it. `undefined` removes a key. Changing anything
 * removes `offset` (a new filter starts on the first page) unless the change sets `offset` itself.
 * The setter replaces the history entry, so filters do not fill the back button.
 */
export function useQueryState(): [URLSearchParams, (changes: Changes) => void] {
  const { query } = useRoute();
  const set = useCallback((changes: Changes) => {
    const next = new URLSearchParams(window.location.search);
    for (const [key, value] of Object.entries(changes)) {
      if (value === undefined) next.delete(key);
      else next.set(key, value);
    }
    if (!('offset' in changes)) next.delete('offset');
    const text = next.toString();
    navigate(window.location.pathname + (text === '' ? '' : `?${text}`), { replace: true });
  }, []);
  return [query, set];
}
