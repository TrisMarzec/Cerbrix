import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';

import { api } from '../api/client';
import type { Status } from '../api/types';
import { Banner } from '../components/Banner';
import { roleWord } from '../lib/format';
import { Link, navigate, useRoute } from '../lib/router';
import { usePolled } from '../lib/poll';
import { useSession } from '../session';
import { NowContext, StatusContext } from './context';
import { PasswordDialog } from './PasswordDialog';
import { resolveRoute } from './routes';
import { TopBar } from './TopBar';

const NOTICE_MS = 8000;

interface StatusAnswer {
  status: Status;
  /** How far the server's clock is ahead of the browser's, in ms; undefined without a Date header. */
  skewMs: number | undefined;
}

/** The status, with the difference between the server's clock (its Date header) and the browser's. */
async function loadStatus(signal: AbortSignal): Promise<StatusAnswer> {
  const answer = await api.getDated<Status>('/status', undefined, signal);
  const skewMs = answer.serverDate === undefined ? undefined : answer.serverDate.getTime() - Date.now();
  return { status: answer.data, skewMs };
}

function unacknowledged(status: Status | undefined): number {
  if (status === undefined) return 0;
  return Object.values(status.problems).reduce((sum, count) => sum + count.unacknowledged, 0);
}

/**
 * Navigation, top bar and the page of the current URL. Owns the one `GET /status` poll and the clock.
 * The clock is the browser's, corrected by the server's Date header of the last status answer, so
 * durations and "x ago" compare server timestamps with the server's time.
 */
export function Shell() {
  const { user, can, logout } = useSession();
  const { path } = useRoute();
  const { data: answer, staleSince, error, reload: reloadStatus } = usePolled(loadStatus, []);
  const status = answer?.status;
  // The last known difference stays while a poll fails.
  const skew = useRef(0);
  if (answer?.skewMs !== undefined) skew.current = answer.skewMs;
  const [tick, setTick] = useState(() => Date.now());
  const now = useMemo(() => new Date(tick + skew.current), [tick, skew.current]);
  const [passwordOpen, setPasswordOpen] = useState(false);
  const [notice, setNotice] = useState<string>();
  const [logoutFailed, setLogoutFailed] = useState(false);
  const main = useRef<HTMLElement>(null);
  const shownPath = useRef(path);

  // A hidden tab's timers are slowed down: becoming visible moves the clock on at once.
  useEffect(() => {
    const id = setInterval(() => {
      setTick(Date.now());
    }, 10_000);
    const onVisibility = () => {
      if (!document.hidden) setTick(Date.now());
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      clearInterval(id);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);

  useLayoutEffect(() => {
    if (path === '/') navigate('/dashboards', { replace: true });
  }, [path]);

  const page = resolveRoute(path);

  useEffect(() => {
    document.title = `${page.title} · Cerbrix`;
  }, [page.title]);

  // After a navigation, keyboard and screen-reader users land on the new page; an old warning about
  // a failed logout is gone.
  useEffect(() => {
    if (shownPath.current !== path) {
      shownPath.current = path;
      setLogoutFailed(false);
      main.current?.focus();
    }
  }, [path]);

  useEffect(() => {
    if (notice === undefined) return undefined;
    const id = setTimeout(() => {
      setNotice(undefined);
    }, NOTICE_MS);
    return () => {
      clearTimeout(id);
    };
  }, [notice]);

  const unacked = unacknowledged(status);
  const entries: Array<{ href: string; label: string; show: boolean }> = [
    { href: '/dashboards', label: 'Dashboards', show: true },
    { href: '/hosts', label: 'Hosts', show: true },
    { href: '/problems', label: 'Problems', show: true },
    { href: '/configuration', label: 'Configuration', show: true },
    { href: '/administration', label: 'Administration', show: can('admin') },
  ];

  return (
    <NowContext.Provider value={now}>
      <StatusContext.Provider value={{ status, staleSince, error, reloadStatus }}>
        <div class="shell">
          <nav class="nav" aria-label="Main">
            <div class="brand">Cerbrix</div>
            {entries
              .filter((entry) => entry.show)
              .map((entry) => (
                <Link
                  key={entry.href}
                  href={entry.href}
                  aria-current={page.nav === entry.href ? 'page' : undefined}
                >
                  {entry.label}
                  {entry.href === '/problems' && unacked > 0 && (
                    <span class="badge num" title={`${unacked} unacknowledged`}>
                      {unacked}
                    </span>
                  )}
                </Link>
              ))}
            <div class="who">
              <p>{`${user.name} · ${roleWord(user.role)}`}</p>
              <button
                type="button"
                class="nav-action"
                onClick={() => {
                  setPasswordOpen(true);
                }}
              >
                Change password
              </button>
              <button
                type="button"
                class="nav-action"
                onClick={() => {
                  setLogoutFailed(false);
                  void logout().then((out) => {
                    if (!out) setLogoutFailed(true);
                  });
                }}
              >
                Log out
              </button>
              {notice !== undefined && <p role="status">{notice}</p>}
            </div>
          </nav>
          <div class="content">
            <TopBar />
            {logoutFailed && (
              <Banner kind="warning">
                Could not log out: the server did not answer. You are still logged in.
              </Banner>
            )}
            <main id="main" tabIndex={-1} ref={main}>
              {page.element}
            </main>
          </div>
        </div>
        {passwordOpen && (
          <PasswordDialog
            userName={user.name}
            onClose={() => {
              setPasswordOpen(false);
            }}
            onChanged={() => {
              setPasswordOpen(false);
              setNotice('Password changed');
            }}
          />
        )}
      </StatusContext.Provider>
    </NowContext.Provider>
  );
}
