import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

import { ApiError } from '../api/client';

interface Polled<T> {
  data: T | undefined;
  error: ApiError | undefined;
  staleSince: Date | undefined;
  reload: () => void;
}

interface State<T> {
  data: T | undefined;
  error: ApiError | undefined;
  staleSince: Date | undefined;
}

function asApiError(e: unknown): ApiError {
  return e instanceof ApiError ? e : new ApiError(0, 'internal', 'Unexpected error');
}

/**
 * Loads at once and then `everyMs` after the previous answer arrived. No request and no timer while
 * the page is hidden; becoming visible loads at once. A request is aborted when the hook unmounts or
 * `deps` change (the data of the old deps stays until the new answer arrives). An error keeps the
 * last data and sets `staleSince` to the first failure; a 401 is not an error here (the client has
 * told `onUnauthorized`). It never throws.
 */
export function usePolled<T>(
  load: (signal: AbortSignal) => Promise<T>,
  deps: unknown[],
  everyMs = 10_000,
): Polled<T> {
  const [state, setState] = useState<State<T>>({
    data: undefined,
    error: undefined,
    staleSince: undefined,
  });
  const loadRef = useRef(load);
  loadRef.current = load;
  const restart = useRef<() => void>(() => {});

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;

    const schedule = () => {
      if (stopped || document.hidden) return;
      timer = setTimeout(run, everyMs);
    };

    function run() {
      clearTimeout(timer);
      controller?.abort();
      if (stopped || document.hidden) return;
      const mine = new AbortController();
      controller = mine;
      loadRef.current(mine.signal).then(
        (data) => {
          if (mine.signal.aborted) return;
          setState({ data, error: undefined, staleSince: undefined });
          schedule();
        },
        (e: unknown) => {
          if (mine.signal.aborted) return;
          const error = asApiError(e);
          if (error.status !== 401) {
            setState((previous) => ({
              data: previous.data,
              error,
              staleSince: previous.staleSince ?? new Date(),
            }));
          }
          schedule();
        },
      );
    }

    const onVisibility = () => {
      if (document.hidden) clearTimeout(timer);
      else run();
    };

    restart.current = run;
    document.addEventListener('visibilitychange', onVisibility);
    run();
    return () => {
      stopped = true;
      clearTimeout(timer);
      controller?.abort();
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [...deps, everyMs]);

  const reload = useCallback(() => {
    restart.current();
  }, []);

  return { ...state, reload };
}
