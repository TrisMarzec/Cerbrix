import './wall.css';

import { useEffect, useState } from 'preact/hooks';

import { api, ApiError, seg, serverSkewMs } from '../api/client';
import type { DashboardDetail, Status } from '../api/types';
import { usePolled } from '../lib/poll';
import { Link } from '../lib/router';
import { useWallNow } from './clock';
import { DashboardView } from './DashboardView';
import { WallFrame } from './WallFrame';

const POLL_MS = 5000;
/** After this long without a successful answer the wall says that the connection is lost. */
const LOST_MS = 15_000;
/** A dashboard opened on this screen was not switched to by an alarm. */
const NO_CAUSES: string[] = [];

function loadDashboard(id: string, signal: AbortSignal): Promise<DashboardDetail> {
  // `.` and `..` cannot be sent as a path segment, and no dashboard has such an id.
  if (id === '.' || id === '..') return Promise.reject(new ApiError(404, 'not_found', 'no such dashboard'));
  return api.get<DashboardDetail>(`/dashboards/${seg(id)}`, undefined, signal);
}

/**
 * Since when the polls fail, once they have failed for `LOST_MS`; undefined before that and after
 * a success. A timer renders again at the moment the limit is reached.
 */
function useLostSince(staleSince: Date | undefined): Date | undefined {
  const [, wake] = useState(0);
  const since = staleSince?.getTime();
  useEffect(() => {
    if (since === undefined) return undefined;
    const wait = since + LOST_MS - Date.now();
    if (wait <= 0) return undefined;
    const timer = setTimeout(() => {
      wake((n) => n + 1);
    }, wait);
    return () => {
      clearTimeout(timer);
    };
  }, [since]);
  if (since === undefined || Date.now() - since < LOST_MS) return undefined;
  // `staleSince` is the browser's time; the wall shows the server's.
  return new Date(since + serverSkewMs());
}

/**
 * A dashboard in wall mode on this screen ("Open on this screen"), for a logged-in user: outside
 * the desk shell, polled every 5 seconds, with the configuration's sync state from `/status`.
 */
export function WallPage({ id }: { id: string }) {
  const now = useWallNow();
  const dashboard = usePolled((signal) => loadDashboard(id, signal), [id], POLL_MS);
  const status = usePolled((signal) => api.get<Status>('/status', undefined, signal), [], POLL_MS);
  const lostSince = useLostSince(dashboard.staleSince);
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

  const sync = status.data?.gitops === true ? status.data.sync : undefined;
  if (dashboard.data === undefined) {
    return (
      <WallFrame title={id} sync={sync} now={now} lostSince={lostSince} back="/dashboards">
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
      back="/dashboards"
    />
  );
}
