import './wall.css';

import { useEffect } from 'preact/hooks';

import { api, ApiError, isApiError, seg } from '../api/client';
import type { DashboardDetail, Status } from '../api/types';
import { usePolled } from '../lib/poll';
import { Link } from '../lib/router';
import { useServerNow } from '../lib/serverNow';
import { CLOCK_MS, POLL_MS } from './constants';
import { DashboardView } from './DashboardView';
import { useLostSince } from './lost';
import { WallRoot } from './WallRoot';

function loadDashboard(id: string, signal: AbortSignal): Promise<DashboardDetail> {
  // `.` and `..` cannot be sent as a path segment, and no dashboard has such an id.
  if (id === '.' || id === '..') return Promise.reject(new ApiError(404, 'not_found', 'no such dashboard'));
  return api.get<DashboardDetail>(`/dashboards/${seg(id)}`, undefined, signal);
}

/**
 * A dashboard in wall mode on this screen ("Open on this screen"), for a logged-in user: outside
 * the desk shell, polled every 5 seconds, with the configuration's sync state from `/status`.
 */
export function WallPage({ id }: { id: string }) {
  const now = useServerNow(CLOCK_MS);
  const dashboard = usePolled((signal) => loadDashboard(id, signal), [id], POLL_MS);
  const status = usePolled((signal) => api.get<Status>('/status', undefined, signal), [], POLL_MS);
  // One rule (R17): 15 s after the last successful answer of each poll.
  const lostSince = useLostSince(dashboard.answeredAt);
  // A sync state that could not be asked for 15 seconds is not shown as current.
  const statusFailing = useLostSince(status.answeredAt) !== undefined;
  const name = dashboard.data?.name;

  useEffect(() => {
    document.title = `${name ?? id} · Cerbrix`;
  }, [name, id]);

  // Only the API's own 404 (R15): a proxy's while the server restarts keeps the picture.
  if (isApiError(dashboard.error, 'not_found')) {
    // Not a wall: a page on the wall's dark background with the way back.
    return (
      <WallRoot>
        <main class="wall-missing">
          <p class="wall-missing-text">There is no dashboard named {id}.</p>
          <p>
            <Link href="/dashboards" class="wall-missing-back">
              Back to the dashboards
            </Link>
          </p>
        </main>
      </WallRoot>
    );
  }

  const sync = !statusFailing && status.data?.gitops === true ? status.data.sync : undefined;
  // The server answered, with an error (status 0: it could not be reached).
  const serverError = dashboard.error !== undefined && dashboard.error.status !== 0;
  // One frame for the life of the page: before the first answer it has the id as title.
  return (
    <DashboardView
      dashboard={dashboard.data}
      title={id}
      sync={sync}
      now={now}
      lostSince={lostSince}
      serverError={serverError}
      back="/dashboards"
    />
  );
}
