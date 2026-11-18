import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

import { ApiError } from '../api/client';

interface Polled<T> {
  data: T | undefined;
  error: ApiError | undefined;
  staleSince: Date | undefined;
  /**
   * True from a change of `deps` until the first answer (success or failure) for the new deps has
   * arrived. While it is true `data` is still the previous deps' data (a page may show it dimmed),
   * and `error` and `staleSince` are cleared. Ordinary polls and `reload()` never set it.
   */
  pending: boolean;
  reload: () => void;
}

interface State<T> {
  data: T | undefined;
  error: ApiError | undefined;
  staleSince: Date | undefined;
  /** The deps generation the last answer belongs to. */
  answered: number;
}

function asApiError(e: unknown): ApiError {
  return e instanceof ApiError ? e : new ApiError(0, 'internal', 'Unexpected error');
}

function sameDeps(a: unknown[], b: unknown[]): boolean {
  return a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
}

/** Load once per mount (and on `reload()`) instead of polling: for lists that rarely change. */
export const ONCE = 'once';

/**
 * Loads at once and then `every` milliseconds after the previous answer arrived. No request and no
 * timer while the page is hidden; becoming visible loads at once. With `every = 'once'` there is no
 * interval: it loads at mount and on `reload()`, and becoming visible loads again only when the
 * last load failed (or none was made). A request is aborted when the hook unmounts or `deps`
 * change. An error (also an exception thrown by `load` itself) keeps the last data and sets
 * `staleSince` to the first failure, except for the first answer after a deps change: a failure
 * there leaves no data. A `403` drops the data as well, on any poll: a role that may no longer read
 * something must not keep seeing the last answer. A 401 is not an error here (the client has told
 * `onUnauthorized`). It never throws.
 */
export function usePolled<T>(
  load: (signal: AbortSignal) => Promise<T>,
  deps: unknown[],
  every: number | typeof ONCE = 10_000,
): Polled<T> {
  const lastDeps = useRef(deps);
  const generation = useRef(0);
  if (!sameDeps(lastDeps.current, deps)) {
    lastDeps.current = deps;
    generation.current += 1;
  }
  const current = generation.current;

  const [state, setState] = useState<State<T>>({
    data: undefined,
    error: undefined,
    staleSince: undefined,
    answered: 0,
  });
  const loadRef = useRef(load);
  loadRef.current = load;
  const restart = useRef<() => void>(() => {});

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;
    /** For `once`: true until a load succeeded. */
    let needed = true;

    const schedule = () => {
      if (stopped || document.hidden || every === ONCE) return;
      timer = setTimeout(run, every);
    };

    function run() {
      clearTimeout(timer);
      controller?.abort();
      if (stopped || document.hidden) return;
      const mine = new AbortController();
      controller = mine;
      let request: Promise<T>;
      try {
        request = loadRef.current(mine.signal);
      } catch (e) {
        request = Promise.reject(e);
      }
      request.then(
        (data) => {
          if (mine.signal.aborted) return;
          needed = false;
          setState({ data, error: undefined, staleSince: undefined, answered: current });
          schedule();
        },
        (e: unknown) => {
          if (mine.signal.aborted) return;
          const error = asApiError(e);
          if (error.status !== 401) {
            needed = true;
            const keep = error.status !== 403;
            setState((previous) =>
              previous.answered === current
                ? {
                    data: keep ? previous.data : undefined,
                    error,
                    staleSince: previous.staleSince ?? new Date(),
                    answered: current,
                  }
                : { data: undefined, error, staleSince: new Date(), answered: current },
            );
          }
          schedule();
        },
      );
    }

    const onVisibility = () => {
      if (document.hidden) clearTimeout(timer);
      else if (every !== ONCE || needed) run();
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
  }, [...deps, every]);

  const reload = useCallback(() => {
    restart.current();
  }, []);

  const pending = state.answered !== current;
  return {
    data: state.data,
    error: pending ? undefined : state.error,
    staleSince: pending ? undefined : state.staleSince,
    pending,
    reload,
  };
}
