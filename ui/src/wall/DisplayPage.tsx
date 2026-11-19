import './wall.css';

import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { ApiError, seg } from '../api/client';
import type { DashboardDetail, DisplayState } from '../api/types';
import { usePolled } from '../lib/poll';
import { useServerNow } from '../lib/serverNow';
import { DashboardView } from './DashboardView';
import { displayGet } from './displayClient';
import { useFailingFor } from './lost';
import { rotationMs, whatToShow, type Rotation, type Shown } from './show';
import { TokenForm } from './TokenForm';
import { POLL_MS, useDisplayState, useNewVersion } from './useDisplay';
import { WallFrame, type WallAlarm } from './WallFrame';

/** The wall's clock moves on this often; it shows minutes. */
const CLOCK_MS = 5000;
/** The cross-fade of a dashboard change (D13). */
const FADE_MS = 400;
/** A dashboard that answered 404 is tried again after this long: it may come back. */
const RETRY_MS = 5 * 60_000;
const NO_CAUSES: string[] = [];

function reloadPage(): void {
  window.location.reload();
}

function reducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

function loadDashboard(id: string, dashboard: string | undefined, signal: AbortSignal) {
  if (dashboard === undefined) return Promise.resolve(undefined);
  // `.` and `..` cannot be sent as a path segment, and no dashboard has such an id.
  if (dashboard === '.' || dashboard === '..') {
    return Promise.reject(new ApiError(404, 'not_found', 'no such dashboard'));
  }
  return displayGet<DashboardDetail>(id, `/dashboards/${seg(dashboard)}`, signal);
}

/** The earlier of two dates; undefined only when both are. */
function earlier(a: Date | undefined, b: Date | undefined): Date | undefined {
  if (a === undefined) return b;
  if (b === undefined) return a;
  return a.getTime() <= b.getTime() ? a : b;
}

/** The band (D10): the target by its name, where the display came from, how many more wait. */
function bandOf(state: DisplayState, shown: Shown): WallAlarm | undefined {
  const target = shown.dashboard;
  if (shown.alarm === undefined || target === undefined) return undefined;
  const alarm: WallAlarm = { showing: state.names[target] ?? target, waiting: shown.alarm.waiting };
  const from = shown.alarm.switchedFrom;
  if (from !== undefined) alarm.switchedFrom = state.names[from] ?? from;
  return alarm;
}

/** What one layer of the display shows: a dashboard, or a sentence in the frame of the display. */
type Content =
  | { key: string; kind: 'dashboard'; id: string; data: DashboardDetail }
  | { key: string; kind: 'text'; text: string }
  | { key: string; kind: 'empty' };

/** A layer as it was drawn: its content and the band and causes it had. */
interface Picture {
  content: Content;
  alarm: WallAlarm | undefined;
  causes: string[];
}

/** The dashboards that answered 404, with the monotonic time of the answer, for one `allowed` list. */
interface Missing {
  allowed: string;
  at: ReadonlyMap<string, number>;
}

interface RunningProps {
  id: string;
  state: DisplayState;
  now: Date;
  /** Since when the state poll fails, once it has failed for 15 s. */
  stateLost: Date | undefined;
  stateError: ApiError | undefined;
}

/**
 * A display that knows its state: which dashboard it shows (rotation and alarm, `whatToShow`), the
 * poll of that dashboard, and the cross-fade when it changes. Until a new dashboard's first answer
 * the old picture stays; during the fade both layers exist, afterwards only the new one.
 */
function RunningDisplay({ id, state, now, stateLost, stateError }: RunningProps) {
  const [wake, setWake] = useState(0);
  const allowedKey = JSON.stringify(state.allowed);
  const [missing, setMissing] = useState<Missing>({ allowed: allowedKey, at: new Map() });

  // The rotation lives in the page (D4), on the monotonic clock; a new `show` setting starts it over.
  const showKey = JSON.stringify(state.show);
  const rotation = useRef<{ show: string; at: Rotation }>({ show: showKey, at: { index: 0, since: performance.now() } });
  const nowMs = performance.now();
  if (rotation.current.show !== showKey) rotation.current = { show: showKey, at: { index: 0, since: nowMs } };
  // A changed `allowed` list tries every dashboard again; others are tried again after RETRY_MS.
  const missingAt = missing.allowed === allowedKey ? missing.at : new Map<string, number>();
  const absent = new Set([...missingAt].filter(([, at]) => nowMs - at < RETRY_MS).map(([d]) => d));
  const shown = whatToShow(state, rotation.current.at, nowMs, absent);
  rotation.current.at = shown.rotation;
  const dashboard = shown.dashboard;

  // Render again when the rotation moves on or a missing dashboard is due to be tried again.
  const every = rotationMs(state.show);
  const due = [...missingAt.values()].map((at) => at + RETRY_MS).filter((at) => at > nowMs);
  if (every !== undefined && Number.isFinite(shown.rotation.since)) due.push(shown.rotation.since + every);
  const next = due.length === 0 ? undefined : Math.min(...due);
  useEffect(() => {
    if (next === undefined) return undefined;
    const timer = setTimeout(() => setWake((n) => n + 1), Math.max(0, next - performance.now()));
    return () => clearTimeout(timer);
  }, [next, wake]);

  const polled = usePolled((signal) => loadDashboard(id, dashboard, signal), [id, dashboard], POLL_MS);
  const answered = !polled.pending;
  const notFound = answered && dashboard !== undefined && polled.error?.status === 404;
  useEffect(() => {
    if (!notFound || dashboard === undefined) return;
    const at = performance.now();
    setMissing((previous) => {
      const kept = previous.allowed === allowedKey ? previous.at : new Map<string, number>();
      return { allowed: allowedKey, at: new Map(kept).set(dashboard, at) };
    });
  }, [notFound, dashboard, allowedKey]);

  // What is in front now. Without an answer for the shown dashboard yet, the last picture stays.
  let content: Content | undefined;
  if (dashboard === undefined) {
    content = { key: 'none', kind: 'text', text: 'This display has no dashboard to show.' };
  } else if (answered && polled.error?.status === 403) {
    content = { key: `forbidden:${dashboard}`, kind: 'text', text: `This display may not show ${dashboard}.` };
  } else if (answered && !notFound && polled.data !== undefined) {
    content = { key: `dashboard:${dashboard}`, kind: 'dashboard', id: dashboard, data: polled.data };
  }
  const last = useRef<Picture | undefined>(undefined);
  const leaving = useRef<Picture | undefined>(undefined);
  content = content ?? last.current?.content ?? { key: 'empty', kind: 'empty' };
  // The band and the highlighted causes belong to the target only.
  const isTarget = content.kind === 'dashboard' && shown.alarm !== undefined && content.id === dashboard;
  const front: Picture = {
    content,
    alarm: isTarget ? bandOf(state, shown) : undefined,
    causes: isTarget ? state.causes : NO_CAUSES,
  };
  const previous = last.current;
  if (previous !== undefined && previous.content.key !== front.content.key) {
    // Once per change: the old picture fades out over the new one fading in, or goes at once. The
    // empty frame before the first answer just makes room.
    leaving.current = previous.content.kind === 'empty' || reducedMotion() ? undefined : previous;
  }
  last.current = front;

  const [, setFaded] = useState(0);
  const leavingKey = leaving.current?.content.key;
  useLayoutEffect(() => {
    if (leavingKey === undefined) return undefined;
    const timer = setTimeout(() => {
      leaving.current = undefined;
      setFaded((n) => n + 1);
    }, FADE_MS);
    return () => clearTimeout(timer);
  }, [leavingKey, front.content.key]);

  // A dashboard that answers 403 or 404 has answered: that is not a lost connection.
  const failing = answered && polled.error !== undefined && polled.error.status !== 403 && !notFound;
  const dashboardLost = useFailingFor(failing ? polled.staleSince : undefined);
  const lostSince = earlier(stateLost, dashboardLost);
  const errors = [stateLost && stateError, dashboardLost && polled.error].filter((e) => e instanceof ApiError);
  // The server answered, with an error (status 0: it could not be reached).
  const serverError = errors.length > 0 && errors.every((e) => e.status !== 0);

  const draw = ({ content: c, alarm, causes }: Picture) =>
    c.kind === 'dashboard' ? (
      <DashboardView
        dashboard={c.data}
        sync={state.sync}
        now={now}
        alarm={alarm}
        causes={causes}
        lostSince={lostSince}
        serverError={serverError}
      />
    ) : (
      <WallFrame title={state.name} sync={state.sync} now={now} lostSince={lostSince} serverError={serverError}>
        {c.kind === 'text' ? <p class="wall-display-text">{c.text}</p> : null}
      </WallFrame>
    );

  const old = leaving.current;
  return (
    <div class="wall wall-layers">
      {old !== undefined && (
        <div key={old.content.key} class="wall-layer is-leaving" aria-hidden="true">
          {draw(old)}
        </div>
      )}
      <div key={front.content.key} class={old !== undefined ? 'wall-layer is-entering' : 'wall-layer'}>
        {draw(front)}
      </div>
    </div>
  );
}

/** A sentence on the wall's dark root, before a display can be shown. */
function Notice({ text, warning = false }: { text: string; warning?: boolean }) {
  return (
    <div class="wall">
      <main class={warning ? 'wall-notice is-warning' : 'wall-notice'}>
        <p class="wall-notice-text">{text}</p>
      </main>
    </div>
  );
}

interface Props {
  id: string;
  /** Reloads the page when a new version of the UI is on the server; for tests. */
  reload?: () => void;
}

/**
 * A wall monitor at `/display/<id>` (concept sections 5.2, 6 and 8.1): no session and no shell. It
 * asks for the display's state with the stored token, or without one (a logged-in user's cookie
 * answers too); a 401 shows the token form, as does `#token` in the address (D12). Then it shows
 * the dashboards of the display, switching to an alarm and back, and says so when the server is
 * away. It takes no input once it runs.
 */
export function DisplayPage({ id, reload = reloadPage }: Props) {
  const now = useServerNow(CLOCK_MS);
  const [form, setForm] = useState<{ message?: string } | undefined>(() =>
    window.location.hash === '#token' ? {} : undefined,
  );
  const [seed, setSeed] = useState<DisplayState>();
  const poll = useDisplayState(id, form === undefined, seed, (withToken) => {
    setForm(withToken ? { message: 'The token of this display is no longer valid.' } : {});
  });
  const stateLost = useFailingFor(poll.staleSince);  useNewVersion(reload, form === undefined && poll.state !== undefined && poll.error === undefined);

  const name = poll.state?.name;
  useEffect(() => {
    document.title = `${name ?? `Wall display ${id}`} · Cerbrix`;
  }, [name, id]);

  if (form !== undefined) {
    return (
      <TokenForm
        id={id}
        message={form.message}
        onStart={(state) => {
          // Started: a reload must not ask again.
          if (window.location.hash !== '') {
            window.history.replaceState(window.history.state, '', window.location.pathname + window.location.search);
          }
          setSeed(state);
          setForm(undefined);
        }}
      />
    );
  }
  // A logged-in user asking for a display that is not (or no longer) in the configuration; a sync
  // may add it, so the poll goes on.
  if (poll.error?.status === 404) return <Notice text={`There is no display named ${id}.`} />;
  if (poll.state === undefined) {
    if (poll.error === undefined) return <div class="wall" />;
    return (
      <Notice
        warning
        text={poll.error.status === 0 ? 'No connection to the server' : 'The server reported an error'}
      />
    );
  }
  return <RunningDisplay id={id} state={poll.state} now={now} stateLost={stateLost} stateError={poll.error} />;
}
