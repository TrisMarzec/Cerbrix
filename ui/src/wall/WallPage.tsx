import './wall.css';

import { useEffect, useRef, useState } from 'preact/hooks';

import { api, ApiError, seg, serverSkewMs } from '../api/client';
import type { DashboardDetail, Status } from '../api/types';
import { usePolled } from '../lib/poll';
import { Link } from '../lib/router';
import { useServerNow } from '../lib/serverNow';
import { DashboardView } from './DashboardView';
import { WallFrame } from './WallFrame';

const POLL_MS = 5000;
/** The wall's clock moves on this often; it shows minutes. */
const CLOCK_MS = 5000;
/** After this long without a successful answer the wall no longer shows a poll's data as current. */
const LOST_MS = 15_000;
/** A dashboard opened on this screen was not switched to by an alarm. */
const NO_CAUSES: string[] = [];

function loadDashboard(id: string, signal: AbortSignal): Promise<DashboardDetail> {
  // `.` and `..` cannot be sent as a path segment, and no dashboard has such an id.
  if (id === '.' || id === '..') return Promise.reject(new ApiError(404, 'not_found', 'no such dashboard'));
  return api.get<DashboardDetail>(`/dashboards/${seg(id)}`, undefined, signal);
}

/**
 * Since when a poll fails (its `staleSince`), once it has failed for `LOST_MS`; undefined before
 * that and after a success. The time that passed is measured on the monotonic clock, so a jump of
 * the browser's clock cannot delay it; a timer renders again when the limit is reached. The time
 * given back is the server's, as the wall's clock.
 */
function useFailingFor(staleSince: Date | undefined): Date | undefined {
  const since = staleSince?.getTime();
  // When this component first saw the current failure, on the monotonic clock.
  const seen = useRef<{ since: number | undefined; at: number }>({ since: undefined, at: 0 });
  if (seen.current.since !== since) seen.current = { since, at: performance.now() };
  const [, wake] = useState(0);
  useEffect(() => {
    if (since === undefined) return undefined;
    const wait = seen.current.at + LOST_MS - performance.now();
    if (wait <= 0) return undefined;
    const timer = setTimeout(() => {
      wake((n) => n + 1);
    }, wait);
    return () => {
      clearTimeout(timer);
    };
  }, [since]);
  if (since === undefined || performance.now() - seen.current.at < LOST_MS) return undefined;
  // `staleSince` is the browser's time; the wall shows the server's.
  return new Date(since + serverSkewMs());
}

/**
 * A dashboard in wall mode on this screen ("Open on this screen"), for a logged-in user: outside
 * the desk shell, polled every 5 seconds, with the configuration's sync state from `/status`.
 */
export function WallPage({ id }: { id: string }) {
  const now = useServerNow(CLOCK_MS);
  const dashboard = usePolled((signal) => loadDashboard(id, signal), [id], POLL_MS);
  const status = usePolled((signal) => api.get<Status>('/status', undefined, signal), [], POLL_MS);
  const lostSince = useFailingFor(dashboard.staleSince);
  // A sync state that could not be asked for 15 seconds is not shown as current.
  const statusFailing = useFailingFor(status.staleSince) !== undefined;
  const name = dashboard.data?.name;

  useEffect(() => {
    document.title = `${name ?? id} · Cerbrix`;
  }, [name, id]);

  if (dashboard.error?.status === 404) {
    // Not a wall: a page on the wall's dark background with the way back.
    return (
      <div class="wall">
        <main class="wall-missing">
          <p class="wall-missing-text">There is no dashboard named {id}.</p>
          <p>
            <Link href="/dashboards" class="wall-missing-back">
              Back to the dashboards
            </Link>
          </p>
        </main>
      </div>
    );
  }

  const sync = !statusFailing && status.data?.gitops === true ? status.data.sync : undefined;
  // The server answered, with an error (status 0: it could not be reached).
  const serverError = dashboard.error !== undefined && dashboard.error.status !== 0;
  if (dashboard.data === undefined) {
    return (
      <WallFrame title={id} sync={sync} now={now} lostSince={lostSince} serverError={serverError} back="/dashboards">
        {null}
      </WallFrame>
    );
  }
  return (
    <DashboardView
      dashboard={dashboard.data}
      sync={sync}
      now={now}
      causes={NO_CAUSES}
      lostSince={lostSince}
      serverError={serverError}
      back="/dashboards"
    />
  );
}
