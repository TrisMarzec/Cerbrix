import './wall.css';

import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { ApiError, isApiError } from '../api/client';
import type { DashboardDetail, DisplayState } from '../api/types';
import { moment, type Moment } from '../lib/poll';
import { useServerNow } from '../lib/serverNow';
import { DashboardGrid } from './DashboardView';
import { tokenKept } from './displayClient';
import { useLostSince } from './lost';
import {
  followShow,
  missingDueAt,
  NO_MISSING,
  noteMissing,
  rotationDueAt,
  stillMissing,
  whatToShow,
  type Missing,
  type Rotation,
  type Shown,
} from './show';
import { TokenForm } from './TokenForm';
import { useDashboardPoll, useDisplayState, useNewVersion } from './useDisplay';
import { WallFrame, type WallAlarm } from './WallFrame';

/** The wall's clock moves on this often; it shows minutes. */
const CLOCK_MS = 5000;
/** The cross-fade of a dashboard change (D13). */
const FADE_MS = 400;
/** How long the previous picture may stand in for a new dashboard that has not answered yet. */
const HANDOVER_MS = 5000;
const NO_CAUSES: string[] = [];

function reloadPage(): void {
  window.location.reload();
}

function reducedMotion(): boolean {
  if (typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
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

/** What one layer of the grid area shows: a dashboard's panels, a sentence, or nothing yet. */
type Layer =
  | { key: string; kind: 'grid'; data: DashboardDetail; causes: string[] }
  | { key: string; kind: 'text'; text: string }
  | { key: 'blank'; kind: 'blank' };

const BLANK: Layer = { key: 'blank', kind: 'blank' };

function text(key: string, sentence: string): Layer {
  return { key, kind: 'text', text: sentence };
}

interface RunningProps {
  id: string;
  state: DisplayState;
  /** The number of the latest state answer. */
  answers: number;
  now: Date;
  /** The time of the state's last answer, once that is 15 s old (R17). */
  stateLost: Date | undefined;
  stateError: ApiError | undefined;
  /** The state was read with a login, not a display token: the header says so (R16). */
  onLogin: boolean;
}

/**
 * A display that knows its state, in one frame for the life of the page (ruling R11): which
 * dashboard it shows (`whatToShow`), the poll of that dashboard, and the cross-fade of the grid
 * area when it changes. The title and the band follow the state at once; the previous picture
 * stands in for a new dashboard only briefly, and the header measures "lost" from the last answer
 * of the picture in front (ruling R12).
 */
function RunningDisplay({ id, state, answers, now, stateLost, stateError, onLogin }: RunningProps) {
  const [wake, setWake] = useState(0);
  const nowMs = performance.now();

  // The rotation lives in the page (D4), on the monotonic clock.
  const kept = useRef<{ show: DisplayState['show']; rotation: Rotation }>({
    show: state.show,
    rotation: { index: 0, since: nowMs },
  });
  const [missing, setMissing] = useState<Missing>(NO_MISSING);
  const rotation = followShow(kept.current.show, state.show, kept.current.rotation, nowMs);
  const shown = whatToShow(state, rotation, nowMs, stillMissing(missing, state, nowMs, answers));
  kept.current = { show: state.show, rotation: shown.rotation };
  const dashboard = shown.dashboard;
  const isTarget = shown.alarm !== undefined;

  const poll = useDashboardPoll(id, dashboard);
  // Only the API's own 404 (R15): a proxy's while the server restarts is an ordinary failure.
  const notFound = dashboard !== undefined && isApiError(poll.error, 'not_found');
  useEffect(() => {
    if (!notFound || dashboard === undefined) return;
    const at = performance.now();
    setMissing((previous) => noteMissing(previous, state, dashboard, at, isTarget ? answers : undefined));
  }, [notFound, dashboard]);

  // What the grid area should show now; undefined: the new dashboard has not answered yet.
  let title = state.name;
  let current: Layer | undefined;
  let handingOver = false;
  if (dashboard === undefined) {
    current = text('none', 'This display has no dashboard to show.');
  } else if (isApiError(poll.error, 'forbidden')) {
    current = text(`forbidden:${dashboard}`, `This display may not show ${dashboard}.`);
  } else {
    title = poll.data?.name ?? state.names[dashboard] ?? dashboard;
    if (poll.data !== undefined) {
      const causes = isTarget ? state.causes : NO_CAUSES;
      current = { key: `grid:${dashboard}`, kind: 'grid', data: poll.data, causes };
    } else if (!notFound && poll.error === undefined && nowMs - poll.since.mono < HANDOVER_MS) {
      handingOver = true;
    } else if (!notFound) {
      current = text(`failed:${dashboard}`, 'This dashboard could not be loaded.');
    }
  }

  // The picture in front and the one fading out under it; at most these two.
  const last = useRef<Layer | undefined>(undefined);
  const leaving = useRef<Layer | undefined>(undefined);
  const previous = last.current;
  // Standing in, the previous picture marks no causes: they belong to the dashboard shown.
  const standIn = previous?.kind === 'grid' ? { ...previous, causes: NO_CAUSES } : previous;
  const front = current ?? standIn ?? BLANK;
  if (previous !== undefined && previous.key !== front.key) {
    // Once per change: the new picture fades in over the old one, which a newer change replaces.
    leaving.current = previous.kind === 'blank' || reducedMotion() ? undefined : previous;
  }
  last.current = front;

  const [, setFaded] = useState(0);
  const leavingKey = leaving.current?.key;
  useLayoutEffect(() => {
    if (leavingKey === undefined) return undefined;
    const timer = setTimeout(() => {
      leaving.current = undefined;
      setFaded((n) => n + 1);
    }, FADE_MS);
    return () => clearTimeout(timer);
  }, [leavingKey, front.key]);

  // The last answer behind the picture in front. Standing in or saying that the dashboard could not
  // be loaded, that is still the previous picture's; with nothing to show there is nothing to age.
  const answered = useRef<Moment>(moment());
  if (dashboard === undefined) {
    answered.current = moment();
  } else if (poll.answeredAt !== undefined && (current?.kind === 'grid' || isApiError(poll.error, 'forbidden'))) {
    answered.current = poll.answeredAt;
  }
  // The one rule (R17): lost 15 s after the last answer of the picture in front.
  const frontLost = useLostSince(answered.current);
  const lostSince = earlier(stateLost, frontLost);
  // "The server reported an error" only when every failing source got an answer with an error; a
  // request still waiting counts as no answer.
  const failing: (ApiError | undefined)[] = [];
  if (stateLost !== undefined) failing.push(stateError);
  if (frontLost !== undefined) failing.push(poll.error);
  const serverError = failing.length > 0 && failing.every((e) => e !== undefined && e.status !== 0);

  // Render again when the rotation moves on, a missing dashboard may be asked for again, the
  // stand-in time is over (the picture in front getting too old is the lost rule's own timer).
  const due = [
    rotationDueAt(state.show, shown.rotation),
    missingDueAt(missing, state, nowMs, answers),
    handingOver ? poll.since.mono + HANDOVER_MS : undefined,
  ].filter((at): at is number => at !== undefined);
  const next = due.length === 0 ? undefined : Math.min(...due);
  useEffect(() => {
    if (next === undefined) return undefined;
    const timer = setTimeout(() => setWake((n) => n + 1), Math.max(0, next - performance.now()));
    return () => clearTimeout(timer);
  }, [next, wake]);

  const draw = (layer: Layer) => {
    if (layer.kind === 'grid') return <DashboardGrid dashboard={layer.data} now={now} causes={layer.causes} />;
    if (layer.kind === 'text') return <p class="wall-display-text">{layer.text}</p>;
    return null;
  };

  const old = leaving.current;
  return (
    <WallFrame
      title={title}
      sync={state.sync}
      now={now}
      alarm={bandOf(state, shown)}
      lostSince={lostSince}
      serverError={serverError}
      onLogin={onLogin}
    >
      <div class="wall-layers">
        {old !== undefined && (
          <div key={old.key} class="wall-layer is-leaving" aria-hidden="true">
            {draw(old)}
          </div>
        )}
        <div key={front.key} class={old !== undefined ? 'wall-layer is-entering' : 'wall-layer'}>
          {draw(front)}
        </div>
      </div>
    </WallFrame>
  );
}

/** A sentence on the wall's dark root, before a display can be shown. */
function Notice({ text: sentence, warning = false }: { text: string; warning?: boolean }) {
  return (
    <div class="wall">
      <main class={warning ? 'wall-notice is-warning' : 'wall-notice'}>
        <p class="wall-notice-text">{sentence}</p>
      </main>
    </div>
  );
}

/** Removes `#token` from the address once the display has started, so a reload does not ask again. */
function dropHash(): void {
  if (window.location.hash === '') return;
  const { pathname, search } = window.location;
  window.history.replaceState(window.history.state, '', pathname + search);
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
  const stateLost = useLostSince(poll.answeredAt);
  // Not while the browser does not keep the token (R18): the reload would lose it.
  useNewVersion(
    reload,
    form === undefined && poll.state !== undefined && poll.error === undefined && tokenKept(id),
  );

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
          dropHash();
          setSeed(state);
          setForm(undefined);
        }}
      />
    );
  }
  // A logged-in user asking for a display that is not (or no longer) in the configuration; a sync
  // may add it, so the poll goes on.
  if (isApiError(poll.error, 'not_found')) return <Notice text={`There is no display named ${id}.`} />;
  if (poll.state === undefined) {
    if (poll.error === undefined) return <div class="wall" />;
    return (
      <Notice
        warning
        text={poll.error.status === 0 ? 'No connection to the server' : 'The server reported an error'}
      />
    );
  }
  return (
    <RunningDisplay
      id={id}
      state={poll.state}
      answers={poll.answers}
      now={now}
      stateLost={stateLost}
      stateError={poll.error}
      onLogin={!poll.withToken}
    />
  );
}
