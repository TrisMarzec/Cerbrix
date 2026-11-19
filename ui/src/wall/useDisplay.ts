import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, isApiError, READ_LIMIT_MS, seg } from '../api/client';
import type { DashboardDetail, DisplayState } from '../api/types';
import { moment, type Moment } from '../lib/poll';
import { POLL_MS } from './constants';
import { displayGet, forgetToken, storedToken } from './displayClient';

/** How often a wall looks for a new version of the UI (D7). */
const VERSION_MS = 60_000;

export interface DisplayPoll {
  /** The last state the server gave; kept while later polls fail. */
  state: DisplayState | undefined;
  /** The error of the last poll; undefined after a success. */
  error: ApiError | undefined;
  /** When the last successful answer (the seed included) arrived; undefined before one. */
  answeredAt: Moment | undefined;
  /** How many successful answers (the seed included) there have been: numbers the state answers. */
  answers: number;
  /** Whether the last successful answer was read with a display token; false: with a login. */
  withToken: boolean;
}

function asApiError(e: unknown): ApiError {
  return e instanceof ApiError ? e : new ApiError(0, 'internal', 'Unexpected error');
}

/**
 * Polls the state of a display while `active`: at once, or, with `seed` (the answer the token form
 * just got), 5 seconds after it; then 5 seconds after each answer, so one request at most is in
 * flight. A failure keeps the last state and never stops the loop, except the API's own 401 (R15):
 * the stored token the request carried is forgotten, `onUnauthorized(withToken)` is called and the
 * loop stops. A 401 without the API's JSON body (a gateway's) is an ordinary failure.
 * Inactive, or on unmount, the request in flight is aborted and no timer is left.
 */
export function useDisplayState(
  id: string,
  active: boolean,
  seed: DisplayState | undefined,
  onUnauthorized: (withToken: boolean) => void,
): DisplayPoll {
  const [poll, setPoll] = useState<DisplayPoll>({
    state: undefined,
    error: undefined,
    answeredAt: undefined,
    answers: 0,
    withToken: false,
  });
  const seedRef = useRef(seed);
  seedRef.current = seed;
  const unauthorized = useRef(onUnauthorized);
  unauthorized.current = onUnauthorized;

  useEffect(() => {
    if (!active) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;

    const schedule = () => {
      if (!stopped) timer = setTimeout(run, POLL_MS);
    };

    function run() {
      const mine = new AbortController();
      controller = mine;
      const sent = storedToken(id);
      displayGet<DisplayState>(id, '', mine.signal, sent).then(
        (state) => {
          if (mine.signal.aborted) return;
          setPoll((previous) => ({
            state,
            error: undefined,
            answeredAt: moment(),
            answers: previous.answers + 1,
            withToken: sent !== undefined,
          }));
          schedule();
        },
        (e: unknown) => {
          if (mine.signal.aborted) return;
          const error = asApiError(e);
          // Only the API's own 401 (R15): a gateway's is an ordinary failure, and the token stays.
          if (isApiError(error, 'unauthorized')) {
            // Only the token this request carried: one entered meanwhile is not touched.
            if (sent !== undefined && storedToken(id) === sent) forgetToken(id);
            unauthorized.current(sent !== undefined);
            return;
          }
          setPoll((previous) => ({
            state: previous.state,
            error,
            answeredAt: previous.answeredAt,
            answers: previous.answers,
            withToken: previous.withToken,
          }));
          schedule();
        },
      );
    }

    const start = seedRef.current;
    if (start !== undefined) {
      // The token form's answer, read with the token it stored.
      setPoll((previous) => ({
        state: start,
        error: undefined,
        answeredAt: moment(),
        answers: previous.answers + 1,
        withToken: true,
      }));
      schedule();
    } else {
      run();
    }
    return () => {
      stopped = true;
      clearTimeout(timer);
      controller?.abort();
    };
  }, [id, active]);

  return poll;
}

export interface DashboardPoll {
  /** The dashboard asked for; undefined asks for none. */
  dashboard: string | undefined;
  /** When asking for this dashboard started. */
  since: Moment;
  /** Its last successful answer; dropped by a 403. */
  data: DashboardDetail | undefined;
  /** When the API last answered for it with something the display shows: a success or a 403. */
  answeredAt: Moment | undefined;
  /** The error of its last request; undefined after a success. */
  error: ApiError | undefined;
}

function fresh(dashboard: string | undefined): DashboardPoll {
  return { dashboard, since: moment(), data: undefined, answeredAt: undefined, error: undefined };
}

function loadDashboard(id: string, dashboard: string, signal: AbortSignal): Promise<DashboardDetail> {
  // `.` and `..` cannot be sent as a path segment, and no dashboard has such an id.
  if (dashboard === '.' || dashboard === '..') {
    return Promise.reject(new ApiError(404, 'not_found', 'no such dashboard'));
  }
  return displayGet<DashboardDetail>(id, `/dashboards/${seg(dashboard)}`, signal);
}

/**
 * Polls the dashboard a display shows: at once when it changes, then 5 seconds after each answer,
 * one request at most in flight. Everything it gives back belongs to `dashboard`: a change starts
 * from nothing in the same render, so a page never takes one dashboard's answer for another's. A
 * 401 of the API changes nothing here (the state poll handles the token); its 403 drops the data.
 * A 401 or 403 without the API's JSON body is an ordinary failure (R15).
 */
export function useDashboardPoll(id: string, dashboard: string | undefined): DashboardPoll {
  const entry = useRef<DashboardPoll>(fresh(dashboard));
  if (entry.current.dashboard !== dashboard) entry.current = fresh(dashboard);
  const [, render] = useState(0);

  useEffect(() => {
    if (dashboard === undefined) return undefined;
    const mine = entry.current;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;
    const settled = () => {
      render((n) => n + 1);
      if (!stopped) timer = setTimeout(run, POLL_MS);
    };

    function run() {
      const request = new AbortController();
      controller = request;
      loadDashboard(id, dashboard ?? '', request.signal).then(
        (data) => {
          if (request.signal.aborted) return;
          mine.data = data;
          mine.error = undefined;
          mine.answeredAt = moment();
          settled();
        },
        (e: unknown) => {
          if (request.signal.aborted) return;
          const error = asApiError(e);
          if (!isApiError(error, 'unauthorized')) {
            mine.error = error;
            if (isApiError(error, 'forbidden')) {
              mine.data = undefined;
              mine.answeredAt = moment();
            }
          }
          settled();
        },
      );
    }

    run();
    return () => {
      stopped = true;
      clearTimeout(timer);
      controller?.abort();
    };
  }, [id, dashboard]);

  return { ...entry.current };
}

/** Set once a reload was asked for: a page reloads itself once at most (D7). */
let reloadAsked = false;

/** Lets the next page reload again; for tests. */
export function resetVersionCheck(): void {
  reloadAsked = false;
}

/** The key in `history.state` that carries the time of the last reload across it (ruling R18). */
const RELOAD_MARK = 'cerbrixReloadAt';
/** Within this long of a reload (wall clock) the loaded page does not reload again. */
const RELOAD_GUARD_MS = 10 * 60_000;

/** The time of the reload that loaded this page, from `history.state`; undefined without one. */
function reloadMark(): number | undefined {
  const state: unknown = window.history.state;
  if (typeof state !== 'object' || state === null) return undefined;
  const at = (state as Record<string, unknown>)[RELOAD_MARK];
  return typeof at === 'number' && Number.isFinite(at) ? at : undefined;
}

/**
 * Records the time of the reload in `history.state`, keeping what is there: unlike a module
 * variable it survives the reload, and unlike storage it is this tab's only.
 */
function markReload(): void {
  const state: unknown = window.history.state;
  const kept = typeof state === 'object' && state !== null ? (state as Record<string, unknown>) : {};
  window.history.replaceState({ ...kept, [RELOAD_MARK]: Date.now() }, '');
}

/**
 * Looks for a new version of the UI (D7): at mount and then every 60 seconds after the previous
 * check, `HEAD /`; the first `ETag` seen is this page's version. When a later one differs and
 * `mayReload` is true at that moment (the latest state poll succeeded, no token form is shown, the
 * browser keeps the token), the time is marked in `history.state` and `reload` is called, once per
 * page life. A page loaded less than 10 minutes after such a mark (wall clock, either way) does
 * not reload at all (ruling R18): a server whose `ETag` changes on every answer cannot make the
 * wall reload again and again. A failed check is ignored.
 */
export function useNewVersion(reload: () => void, mayReload: boolean): void {
  const may = useRef(mayReload);
  may.current = mayReload;
  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  useEffect(() => {
    let first: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let limit: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let stopped = false;
    const mark = reloadMark();
    // Read at the load: this page is the one a recent reload brought.
    const reloadedLately = mark !== undefined && Math.abs(Date.now() - mark) < RELOAD_GUARD_MS;

    const seen = (tag: string) => {
      if (first === undefined) {
        first = tag;
      } else if (tag !== first && may.current && !reloadAsked && !reloadedLately) {
        reloadAsked = true;
        markReload();
        reloadRef.current();
      }
    };

    function check() {
      const mine = new AbortController();
      controller = mine;
      limit = setTimeout(() => mine.abort(), READ_LIMIT_MS);
      fetch('/', { method: 'HEAD', cache: 'no-store', signal: mine.signal })
        .then(
          (response) => {
            const tag = response.headers?.get('ETag') ?? null;
            if (!mine.signal.aborted && response.ok && tag !== null) seen(tag);
          },
          () => {
            // The server may be away or restarting: the next check tries again.
          },
        )
        .finally(() => {
          clearTimeout(limit);
          if (!stopped) timer = setTimeout(check, VERSION_MS);
        });
    }

    check();
    return () => {
      stopped = true;
      clearTimeout(timer);
      clearTimeout(limit);
      controller?.abort();
    };
  }, []);
}
