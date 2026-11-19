import { fireEvent, render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';
import { vi } from 'vitest';

import { resetServerSkew } from '../api/client';
import type { DisplayState } from '../api/types';
import { NOW, criticalProblem, dashboardDetailFixture, statusFixture } from '../test/fixtures';
import { mockFetch, type Answer, type FetchMock, type Responder } from '../test/mockFetch';
import { advance } from '../test/timers';
import { forgetToken, storedToken, storeToken } from './displayClient';
import { DisplayPage } from './DisplayPage';
import { installMeasure, releaseMeasure, type Measure } from './test/measure';
import { resetVersionCheck } from './useDisplay';

vi.mock('uplot', () => ({
  default: class {
    constructor(_opts: uPlot.Options) {}
    setSize() {}
    setData() {}
    destroy() {}
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

const ID = 'noc-left';
const TOKEN = 'cbx_dt_7Hq2-secret';
const STATE = `GET /display/${ID}`;
const dash = (id: string) => `GET /display/${ID}/dashboards/${id}`;
const UNAUTHORIZED: Answer = { status: 401, json: { error: 'unauthorized', message: 'not logged in' } };
const NOT_FOUND: Answer = { status: 404, json: { error: 'not_found', message: 'no such display' } };

const innerWidth = Object.getOwnPropertyDescriptor(window, 'innerWidth');

let mock: FetchMock;
let measure: Measure;
let reload: ReturnType<typeof vi.fn<() => void>>;

beforeEach(() => {
  // `performance` is faked with the timers: the rotation and the 15 s run on the monotonic clock.
  vi.useFakeTimers({
    toFake: [
      'setTimeout',
      'clearTimeout',
      'setInterval',
      'clearInterval',
      'Date',
      'performance',
      'requestAnimationFrame',
      'cancelAnimationFrame',
    ],
  });
  vi.setSystemTime(NOW);
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1920 });
  measure = installMeasure({
    bodyWidth: 1000,
    bodyHeight: 600,
    rowHeight: 100,
    rowWidth: 400,
    summaryHeight: 60,
    emptyWidth: 500,
    emptyHeight: 60,
    lineHeight: 40,
    square: 30,
    gap: 6,
    charWidth: 10,
  });
  reload = vi.fn<() => void>();
  window.history.replaceState(null, '', `/display/${ID}`);
});

afterEach(() => {
  mock.restore();
  releaseMeasure();
  vi.restoreAllMocks();
  resetServerSkew();
  resetVersionCheck();
  forgetToken(ID);
  window.localStorage.clear();
  if (innerWidth !== undefined) Object.defineProperty(window, 'innerWidth', innerWidth);
  vi.useRealTimers();
  window.history.replaceState(null, '', '/');
  document.title = '';
});

function displayState(overrides: Partial<DisplayState> = {}): DisplayState {
  return {
    id: ID,
    name: 'NOC left',
    show: { dashboard: 'a' },
    switch_on_alarm: { min_severity: 'critical' },
    allowed: ['a', 'b', 'c', 'x'],
    names: { a: 'Alpha', b: 'Beta', c: 'Gamma', x: 'Xray' },
    target: null,
    causes: [],
    waiting: [],
    sync: statusFixture().sync,
    ...overrides,
  };
}

const ROTATING = displayState({ show: { rotate: ['a', 'b'], every: '30s' } });

/** A dashboard with the four panels of the fixture; its value panel shows `<word>.load`. */
function board(id: string, name: string, word: string) {
  const panels = dashboardDetailFixture().panels.map((p) => (p.type === 'value' ? { ...p, item: `${word}.load` } : p));
  return dashboardDetailFixture({ id, name, panels });
}

const alpha = board('a', 'Alpha', 'alpha');
const beta = board('b', 'Beta', 'beta');
const gamma = board('c', 'Gamma', 'gamma');
const xray = board('x', 'Xray', 'xray');

/** An answer only for a request with the display's token, as the server gives it; 401 otherwise. */
function withToken(answer: Answer, token = TOKEN): Responder {
  return (request) => (request.headers.get('Authorization') === `Bearer ${token}` ? answer : UNAUTHORIZED);
}

/** Lets the requests answer and the effects they cause run, without moving the clock. */
async function settle() {
  for (let i = 0; i < 4; i += 1) await advance(0);
}

/**
 * Moves the clock in steps of `step` ms, letting everything settle after each: `act` renders only
 * at its end, so one long step would skip the renders (and the timers they set) in between.
 */
async function walk(ms: number, step = 1000) {
  for (let done = 0; done < ms; done += step) {
    await advance(Math.min(step, ms - done));
    await settle();
  }
}

async function open(routes: Record<string, Responder | Responder[]>, { token }: { token?: string } = {}) {
  if (token !== undefined) storeToken(ID, token);
  mock = mockFetch(routes);
  const view = render(<DisplayPage id={ID} reload={reload} />);
  await settle();
  return view;
}

/** The title in the display's one frame. */
function title(): string {
  return document.querySelector('.wall-title')?.textContent ?? '';
}

function layers(): Element[] {
  return [...document.querySelectorAll('.wall-layer')];
}

/** The panels of the picture in front (not of the one under it during a fade). */
function frontPanels(): Element[] {
  return [...document.querySelectorAll('.wall-layer:not(.is-leaving) .wall-panel')];
}

/** Whose panels are in front: each fixture dashboard's value panel names its own item. */
function frontShows(): string | undefined {
  const label = document.querySelector('.wall-layer:not(.is-leaving) .wall-value-body')?.closest('.wall-panel');
  return /db-01 · (\w+)\.load/.exec(label?.textContent ?? '')?.[1];
}

function dimmed(): boolean {
  return document.querySelector('.wall-body')?.classList.contains('is-lost') === true;
}

function band(): string | null {
  return document.querySelector('.wall-band.is-alarm')?.textContent ?? null;
}

function tokenInput(): HTMLInputElement {
  return screen.getByLabelText('Display token');
}

async function enter(token: string) {
  const input = tokenInput();
  input.value = token;
  fireEvent.input(input);
  fireEvent.click(screen.getByRole('button', { name: 'Start' }));
  await settle();
}

function focusables(): Element[] {
  return [...document.querySelectorAll('a[href], button, input, select, textarea, [tabindex]')];
}

describe('DisplayPage: the token', () => {
  it('asks for the token when the server answers 401 without one', async () => {
    const { container } = await open({ [STATE]: withToken({ json: displayState() }) });
    const asked = mock.requests.filter((r) => r.path === `/display/${ID}`);
    expect(asked).toHaveLength(1);
    expect(asked[0]?.headers.has('Authorization')).toBe(false);
    expect(screen.getByRole('heading', { name: `Wall display ${ID}` })).toBeInTheDocument();
    const input = tokenInput();
    // A plain text field: a kiosk browser offers to save a password field after the start (R13).
    expect(input).toHaveAttribute('type', 'text');
    expect(input).toHaveAttribute('inputmode', 'text');
    expect(input).toHaveAttribute('autocomplete', 'off');
    expect(input).toHaveAttribute('autocapitalize', 'off');
    expect(input).toHaveAttribute('autocorrect', 'off');
    expect(input).toHaveAttribute('spellcheck', 'false');
    expect(screen.getByRole('button', { name: 'Start' })).toBeInTheDocument();
    expect(document.activeElement).toBe(input);
    // The form is on the wall's dark root and is all that can be focused.
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(focusables()).toEqual([input, screen.getByRole('button', { name: 'Start' })]);
  });

  it('says that a wrong token is not valid, keeps the focus in the field and stores nothing', async () => {
    await open({ [STATE]: withToken({ json: displayState() }) });
    await enter('cbx_dt_wrong');
    expect(screen.getByText('This token is not valid for this display.')).toBeInTheDocument();
    expect(document.activeElement).toBe(tokenInput());
    expect(storedToken(ID)).toBeUndefined();
  });

  it("does not call a token invalid on a gateway's plain-text 401", async () => {
    await open({ [STATE]: [UNAUTHORIZED, { status: 401, text: 'Unauthorized' }] });
    await enter(TOKEN);
    expect(screen.queryByText('This token is not valid for this display.')).not.toBeInTheDocument();
    expect(screen.getByText('The server reported an error.')).toBeInTheDocument();
  });

  it('says that the server could not be reached when checking the token fails', async () => {
    await open({ [STATE]: [UNAUTHORIZED, 'network'] });
    await enter(TOKEN);
    expect(screen.getByText('Could not reach the server.')).toBeInTheDocument();
    expect(document.activeElement).toBe(tokenInput());
  });

  it('stores a right token and shows the dashboard, sending the token only as a bearer header', async () => {
    await open({ [STATE]: withToken({ json: displayState() }), [dash('a')]: withToken({ json: alpha }) });
    const before = mock.requests.length;
    await enter(TOKEN);
    expect(storedToken(ID)).toBe(TOKEN);
    expect(title()).toBe('Alpha');
    expect(screen.queryByLabelText('Display token')).not.toBeInTheDocument();
    // Once the display runs, nothing can be focused.
    expect(focusables()).toEqual([]);
    await advance(5000);
    expect(mock.count(dash('a'))).toBe(2);
    for (const r of mock.requests) expect(`${r.path}?${r.query.toString()}`).not.toContain(TOKEN);
    for (const r of mock.requests.slice(before).filter((r) => r.path.startsWith('/display/'))) {
      expect(r.headers.get('Authorization')).toBe(`Bearer ${TOKEN}`);
    }
  });

  it('starts with a stored token without asking', async () => {
    await open(
      { [STATE]: withToken({ json: displayState() }), [dash('a')]: withToken({ json: alpha }) },
      { token: TOKEN },
    );
    expect(title()).toBe('Alpha');
    expect(screen.queryByLabelText('Display token')).not.toBeInTheDocument();
  });

  it('shows the dashboard to a logged-in user without a token', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    expect(title()).toBe('Alpha');
    expect(screen.queryByLabelText('Display token')).not.toBeInTheDocument();
    expect(mock.requests.every((r) => !r.headers.has('Authorization'))).toBe(true);
  });

  it('shows the form for #token even when the stored token works, and removes the #token after the start', async () => {
    window.history.replaceState(null, '', `/display/${ID}#token`);
    const fresh = 'cbx_dt_fresh';
    await open(
      { [STATE]: withToken({ json: displayState() }, fresh), [dash('a')]: withToken({ json: alpha }, fresh) },
      { token: TOKEN },
    );
    expect(tokenInput()).toBeInTheDocument();
    expect(mock.count(STATE)).toBe(0);
    await enter(fresh);
    expect(title()).toBe('Alpha');
    expect(storedToken(ID)).toBe(fresh);
    expect(window.location.hash).toBe('');
    expect(window.location.pathname).toBe(`/display/${ID}`);
  });

  it('keeps the old token when a wrong one is entered after #token', async () => {
    window.history.replaceState(null, '', `/display/${ID}#token`);
    await open({ [STATE]: withToken({ json: displayState() }) }, { token: TOKEN });
    await enter('cbx_dt_wrong');
    expect(screen.getByText('This token is not valid for this display.')).toBeInTheDocument();
    expect(storedToken(ID)).toBe(TOKEN);
  });

  it('says for 10 seconds that this browser does not keep the token, then starts with it', async () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('The quota has been exceeded.', 'QuotaExceededError');
    });
    await open({ [STATE]: withToken({ json: displayState() }), [dash('a')]: withToken({ json: alpha }) });
    await enter(TOKEN);
    expect(
      screen.getByText('This browser does not keep the token. It must be entered again after a restart.'),
    ).toBeInTheDocument();
    expect(title()).toBe('');
    await advance(9999);
    expect(title()).toBe('');
    await advance(1);
    await settle();
    expect(title()).toBe('Alpha');
    expect(mock.requests.at(-1)?.headers.get('Authorization')).toBe(`Bearer ${TOKEN}`);
  });

  it('forgets a token that stops working and asks again, saying why', async () => {
    await open(
      { [STATE]: withToken({ json: displayState() }), [dash('a')]: withToken({ json: alpha }) },
      { token: TOKEN },
    );
    expect(title()).toBe('Alpha');
    mock.on(STATE, UNAUTHORIZED);
    await advance(5000);
    await settle();
    expect(screen.getByText('The token of this display is no longer valid.')).toBeInTheDocument();
    expect(tokenInput()).toBeInTheDocument();
    expect(storedToken(ID)).toBeUndefined();
    // No more polls while the form waits.
    const before = mock.count(STATE);
    await advance(30_000);
    expect(mock.count(STATE)).toBe(before);
  });
});

describe('DisplayPage: before the display can be shown', () => {
  it('says that there is no such display, and asks again every 5 seconds', async () => {
    const { container } = await open({ [STATE]: NOT_FOUND });
    expect(screen.getByText(`There is no display named ${ID}.`)).toBeInTheDocument();
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(focusables()).toEqual([]);
    await advance(5000);
    expect(mock.count(STATE)).toBe(2);
    mock.on(STATE, { json: displayState() });
    mock.on(dash('a'), { json: alpha });
    await advance(5000);
    await settle();
    expect(title()).toBe('Alpha');
  });

  it('says that the server cannot be reached on the dark root, and starts once it can', async () => {
    const { container } = await open({ [STATE]: 'network' });
    expect(screen.getByText('No connection to the server')).toBeInTheDocument();
    expect(container.querySelector('.wall')).not.toBeNull();
    await advance(5000);
    expect(mock.count(STATE)).toBe(2);
    mock.on(STATE, { json: displayState() });
    mock.on(dash('a'), { json: alpha });
    await advance(5000);
    await settle();
    expect(title()).toBe('Alpha');
  });

  it('shows the dark root and nothing else until the first answer', async () => {
    const { container } = await open({ [STATE]: () => new Promise(() => {}) });
    expect(container.querySelector('.wall')).not.toBeNull();
    expect(container.textContent).toBe('');
  });

  it('says that there is no display any more when it leaves the configuration (logged-in user)', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(STATE, NOT_FOUND);
    await advance(5000);
    expect(screen.getByText(`There is no display named ${ID}.`)).toBeInTheDocument();
  });
});

describe('DisplayPage: what it shows', () => {
  it('names the page after the display and passes the sync state to the frame', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    expect(document.title).toBe('NOC left · Cerbrix');
    expect(screen.getByText('In sync')).toBeInTheDocument();
  });

  it('asks for the state and the shown dashboard every 5 seconds', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    expect(mock.count(STATE)).toBe(1);
    expect(mock.count(dash('a'))).toBe(1);
    await advance(4999);
    expect(mock.count(STATE)).toBe(1);
    expect(mock.count(dash('a'))).toBe(1);
    await advance(1);
    expect(mock.count(STATE)).toBe(2);
    expect(mock.count(dash('a'))).toBe(2);
  });

  it('rotates through its dashboards on the interval, wrapping at the end', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    expect(title()).toBe('Alpha');
    await advance(29_999);
    expect(title()).toBe('Alpha');
    expect(mock.count(dash('b'))).toBe(0);
    await advance(1);
    await settle();
    // Asked for at once, not at the next poll.
    expect(mock.count(dash('b'))).toBe(1);
    expect(title()).toBe('Beta');
    await advance(30_000);
    await settle();
    expect(title()).toBe('Alpha');
  });

  it('restarts the rotation at its first dashboard when the setting changes', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    await advance(30_000);
    await settle();
    expect(title()).toBe('Beta');
    mock.on(STATE, { json: { ...ROTATING, show: { rotate: ['a', 'b'], every: '1m' } } });
    await advance(5000);
    await settle();
    expect(title()).toBe('Alpha');
    await advance(59_000);
    expect(title()).toBe('Alpha');
    await advance(1000);
    await settle();
    expect(title()).toBe('Beta');
  });

  it('switches to the alarm target at the next poll, with the band and the causes, and goes back when it clears', async () => {
    const alarm = displayState({ target: 'b', causes: [criticalProblem.id], waiting: ['x'] });
    const { container } = await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      [dash('b')]: { json: beta },
    });
    expect(title()).toBe('Alpha');
    expect(band()).toBeNull();
    expect(container.querySelector('.wall-problem.is-cause')).toBeNull();

    mock.on(STATE, { json: alarm });
    await advance(5000);
    await settle();
    expect(title()).toBe('Beta');
    expect(band()).toBe('Alarm: showing Beta. Switched from Alpha. 1 more dashboard waiting.');
    const front = document.querySelector('.wall-layer:not(.is-leaving)');
    expect(front?.querySelectorAll('.wall-problem.is-cause:not(.wall-sample)')).toHaveLength(1);

    mock.on(STATE, { json: displayState() });
    await advance(5000);
    await settle();
    expect(title()).toBe('Alpha');
    expect(band()).toBeNull();
    await advance(400);
    expect(container.querySelector('.wall-problem.is-cause')).toBeNull();
  });

  it('uses the id in the band for a dashboard without a name, and says nothing of a switch to the shown one', async () => {
    await open({
      [STATE]: { json: displayState({ target: 'a', names: {}, waiting: ['b', 'c'] }) },
      [dash('a')]: { json: alpha },
    });
    expect(band()).toBe('Alarm: showing a. 2 more dashboards waiting.');
  });

  it('keeps the rotation where it was during an alarm and gives the interrupted dashboard its full time', async () => {
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: { json: beta },
      [dash('x')]: { json: dashboardDetailFixture({ id: 'x', name: 'Xray' }) },
    });
    await advance(30_000);
    await settle();
    expect(title()).toBe('Beta');
    // 10 s into Beta an alarm comes, and lasts a minute.
    await advance(5000);
    mock.on(STATE, { json: { ...ROTATING, target: 'x' } });
    await advance(5000);
    await settle();
    expect(title()).toBe('Xray');
    expect(band()).toBe('Alarm: showing Xray. Switched from Beta.');
    await advance(60_000);
    expect(title()).toBe('Xray');
    mock.on(STATE, { json: ROTATING });
    await advance(5000);
    await settle();
    expect(title()).toBe('Beta');
    await advance(29_000);
    expect(title()).toBe('Beta');
    await advance(1000);
    await settle();
    expect(title()).toBe('Alpha');
  });

  it('names the new dashboard at once and lets the old grid stand in while the new one is slow (4 s)', async () => {
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: () => new Promise<Answer>((resolve) => setTimeout(() => resolve({ json: beta }), 4000)),
    });
    await advance(30_000);
    await settle();
    expect(mock.count(dash('b'))).toBe(1);
    // The title comes from the state's names, before the answer.
    expect(title()).toBe('Beta');
    expect(frontShows()).toBe('alpha');
    expect(layers()).toHaveLength(1);
    await walk(3000);
    expect(frontShows()).toBe('alpha');
    expect(screen.queryByText('This dashboard could not be loaded.')).not.toBeInTheDocument();
    await walk(1000);
    expect(title()).toBe('Beta');
    expect(frontShows()).toBe('beta');
    expect(dimmed()).toBe(false);
  });

  it('stops standing in after 5 s without an answer and says the dashboard could not be loaded', async () => {
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: () => new Promise<Answer>(() => {}),
    });
    await advance(30_000);
    await settle();
    await walk(4000);
    expect(frontShows()).toBe('alpha');
    await walk(1000);
    await advance(400);
    expect(title()).toBe('Beta');
    expect(screen.getByText('This dashboard could not be loaded.')).toBeInTheDocument();
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(0);
  });

  it('never shows the old picture for a new dashboard that fails, and measures "lost" from the last answer', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: 'network' });
    // In steps, so the page renders each of Alpha's answers as a browser would.
    await walk(30_000, 5000);
    await advance(400);
    // At once: the new name and the line, nothing of Alpha.
    expect(title()).toBe('Beta');
    expect(screen.getByText('This dashboard could not be loaded.')).toBeInTheDocument();
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(0);
    expect(dimmed()).toBe(false);
    // Alpha answered last at 25 s or 30 s; 15 s later the header says so, whichever dashboard is asked for.
    await walk(9000);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await walk(6000);
    expect(screen.getByText(/No connection to the server since/)).toHaveTextContent(
      'No connection to the server since 14:30',
    );
    expect(dimmed()).toBe(true);
    // Recovery: Beta answers.
    mock.on(dash('b'), { json: beta });
    await walk(5000);
    expect(frontShows()).toBe('beta');
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    expect(dimmed()).toBe(false);
  });

  it('shows the alarm target by name with the band when its dashboard fails (500), not the previous picture', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      [dash('x')]: { status: 500, json: { error: 'internal', message: 'boom' } },
    });
    expect(frontShows()).toBe('alpha');
    mock.on(STATE, { json: displayState({ target: 'x', causes: [criticalProblem.id] }) });
    await advance(5000);
    await settle();
    await advance(400);
    expect(title()).toBe('Xray');
    expect(band()).toBe('Alarm: showing Xray. Switched from Alpha.');
    expect(screen.getByText('This dashboard could not be loaded.')).toBeInTheDocument();
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(0);
    // The server answers, with an error: after 15 s without a picture's answer the header says so.
    await walk(15_000);
    expect(screen.getByText(/^The server reported an error since/)).toBeInTheDocument();
    expect(dimmed()).toBe(true);
  });

  it('asks for an alarm target that answered 404 again with the next state that still names it', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      [dash('x')]: [{ status: 404, json: { error: 'not_found', message: 'no such dashboard' } }, { json: xray }],
    });
    mock.on(STATE, { json: displayState({ target: 'x' }) });
    await advance(5000);
    await settle();
    // Gone for now: the rotation is shown, without a band.
    expect(mock.count(dash('x'))).toBe(1);
    expect(title()).toBe('Alpha');
    expect(band()).toBeNull();
    // The next state still names it: asked for again (not after five minutes), and shown.
    await walk(5000);
    expect(mock.count(dash('x'))).toBe(2);
    expect(title()).toBe('Xray');
    expect(band()).toBe('Alarm: showing Xray. Switched from Alpha.');
    await advance(400);
    expect(frontShows()).toBe('xray');
  });

  it('says in the frame that the display may not show a dashboard (403), and moves on at its interval', async () => {
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: { status: 403, json: { error: 'forbidden', message: 'this display may not show that dashboard' } },
    });
    await advance(30_000);
    await settle();
    expect(title()).toBe('NOC left');
    expect(screen.getByText('This display may not show b.')).toBeInTheDocument();
    // An answer, not a lost connection.
    await advance(20_000);
    expect(screen.queryByText(/No connection|reported an error/)).not.toBeInTheDocument();
    await advance(10_000);
    await settle();
    expect(title()).toBe('Alpha');
  });

  it('skips a dashboard that does not exist (404) at once, and tries it again after 5 minutes', async () => {
    const three = { ...ROTATING, show: { rotate: ['a', 'b', 'c'], every: '30s' } };
    await open({
      [STATE]: { json: three },
      [dash('a')]: { json: alpha },
      [dash('b')]: { status: 404, json: { error: 'not_found', message: 'no such dashboard' } },
      [dash('c')]: { json: gamma },
    });
    await advance(30_000);
    await settle();
    expect(mock.count(dash('b'))).toBe(1);
    expect(title()).toBe('Gamma');
    // Gamma gets its full interval, then Alpha, then Gamma again: b is skipped.
    await advance(29_000);
    expect(title()).toBe('Gamma');
    await advance(1000);
    await settle();
    expect(title()).toBe('Alpha');
    await advance(30_000);
    await settle();
    expect(title()).toBe('Gamma');
    expect(mock.count(dash('b'))).toBe(1);
    // Missing since 30 s: tried again when the rotation reaches it after 330 s.
    await walk(235_000, 5000);
    expect(mock.count(dash('b'))).toBe(1);
    await walk(5000, 5000);
    expect(mock.count(dash('b'))).toBe(2);
  });

  it('tries a dashboard that did not exist again when the allowed list changes', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: [{ status: 404, json: { error: 'not_found', message: 'no such dashboard' } }, { json: alpha }],
    });
    expect(screen.getByText('This display has no dashboard to show.')).toBeInTheDocument();
    expect(title()).toBe('NOC left');
    mock.on(STATE, { json: displayState({ allowed: ['a', 'b', 'c'] }) });
    await advance(5000);
    await settle();
    expect(title()).toBe('Alpha');
  });
});

describe("DisplayPage: only the API's own answers decide (R15)", () => {
  // A reverse proxy answers so for every path while the server container restarts.
  const PROXY_404: Answer = { status: 404, text: '404 page not found' };
  const PROXY_401: Answer = { status: 401, text: 'Unauthorized' };
  const API_404: Answer = { status: 404, json: { error: 'not_found', message: 'no such dashboard' } };

  it('keeps the picture on a plain-text 404 of the state, and says after 15 s that the server reported an error', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(STATE, PROXY_404);
    mock.on(dash('a'), PROXY_404);
    await walk(14_000);
    expect(screen.queryByText(`There is no display named ${ID}.`)).not.toBeInTheDocument();
    expect(title()).toBe('Alpha');
    expect(frontShows()).toBe('alpha');
    expect(dimmed()).toBe(false);
    await walk(1000);
    expect(screen.getByText(/^The server reported an error since/)).toHaveTextContent(
      'The server reported an error since 14:30',
    );
    expect(dimmed()).toBe(true);
    expect(frontShows()).toBe('alpha');
  });

  it('says that the server reported an error, not that there is no such display, for a plain-text 404 at the start', async () => {
    await open({ [STATE]: PROXY_404 });
    expect(screen.queryByText(`There is no display named ${ID}.`)).not.toBeInTheDocument();
    expect(screen.getByText('The server reported an error')).toBeInTheDocument();
  });

  it('does not take a plain-text 404 of a dashboard for a missing one: the rotation does not skip it', async () => {
    const three = { ...ROTATING, show: { rotate: ['a', 'b', 'c'], every: '30s' } };
    await open({
      [STATE]: { json: three },
      [dash('a')]: { json: alpha },
      [dash('b')]: PROXY_404,
      [dash('c')]: { json: gamma },
    });
    await advance(30_000);
    await settle();
    await advance(400);
    // Not skipped to Gamma: Beta is named, could not be loaded, and is asked for again.
    expect(title()).toBe('Beta');
    expect(screen.getByText('This dashboard could not be loaded.')).toBeInTheDocument();
    expect(mock.count(dash('c'))).toBe(0);
    await walk(5000);
    expect(mock.count(dash('b'))).toBe(2);
    expect(title()).toBe('Beta');
  });

  it('keeps the picture of the shown dashboard on a plain-text 404 of it', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(dash('a'), PROXY_404);
    await walk(10_000);
    expect(frontShows()).toBe('alpha');
    expect(screen.queryByText('This display has no dashboard to show.')).not.toBeInTheDocument();
    await walk(5000);
    expect(screen.getByText(/^The server reported an error since/)).toBeInTheDocument();
    expect(frontShows()).toBe('alpha');
  });

  it('does not forget the token on a plain-text 401, and keeps the picture', async () => {
    await open(
      { [STATE]: withToken({ json: displayState() }), [dash('a')]: withToken({ json: alpha }) },
      { token: TOKEN },
    );
    mock.on(STATE, PROXY_401);
    mock.on(dash('a'), PROXY_401);
    await walk(15_000);
    expect(storedToken(ID)).toBe(TOKEN);
    expect(screen.queryByLabelText('Display token')).not.toBeInTheDocument();
    expect(frontShows()).toBe('alpha');
    expect(screen.getByText(/^The server reported an error since/)).toBeInTheDocument();
    // The polls go on, and the display is back when the server is.
    mock.on(STATE, withToken({ json: displayState() }));
    mock.on(dash('a'), withToken({ json: alpha }));
    await walk(5000);
    expect(screen.queryByText(/reported an error/)).not.toBeInTheDocument();
  });

  it('asks for a target that is also a held-out rotation entry again with the next state', async () => {
    const three = { ...ROTATING, show: { rotate: ['a', 'b', 'c'], every: '30s' } };
    await open({
      [STATE]: { json: three },
      [dash('a')]: { json: alpha },
      [dash('b')]: API_404,
      [dash('c')]: { json: gamma },
    });
    await advance(30_000);
    await settle();
    // b does not exist: skipped, and held out as a rotation entry for five minutes.
    expect(mock.count(dash('b'))).toBe(1);
    expect(title()).toBe('Gamma');
    // The newest state names b as the target: asked for with it, not after five minutes.
    mock.on(STATE, { json: { ...three, target: 'b' } });
    await walk(5000);
    expect(mock.count(dash('b'))).toBe(2);
    // Still not there: held out until the next state, which asks for it again.
    await walk(5000);
    expect(mock.count(dash('b'))).toBe(3);
    mock.on(dash('b'), { json: beta });
    await walk(5000);
    expect(title()).toBe('Beta');
    expect(band()).toBe('Alarm: showing Beta. Switched from Gamma.');
  });

  it('tries the dashboards again after 5 s when holding them out leaves nothing to show', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: [API_404, { json: alpha }] });
    expect(screen.getByText('This display has no dashboard to show.')).toBeInTheDocument();
    expect(mock.count(dash('a'))).toBe(1);
    await advance(4999);
    await settle();
    expect(mock.count(dash('a'))).toBe(1);
    await advance(1);
    await settle();
    expect(mock.count(dash('a'))).toBe(2);
    expect(title()).toBe('Alpha');
  });
});

describe('DisplayPage: the dashboard change', () => {
  async function rotateToBeta() {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    const ofAlpha = measure.observers().filter((o) => !o.disconnected);
    expect(ofAlpha.length).toBeGreaterThan(0);
    await advance(30_000);
    await settle();
    return ofAlpha;
  }

  it('fades the new grid in over the old one in 400 ms, in the same frame, then keeps only the new one', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    const frame = document.querySelector('.wall');
    const header = document.querySelector('.wall-header');
    const alphaLayer = layers()[0];
    const ofAlpha = measure.observers().filter((o) => !o.disconnected);
    expect(ofAlpha.length).toBeGreaterThan(0);
    const created = measure.observers().length;
    await advance(30_000);
    await settle();

    expect(layers()).toHaveLength(2);
    const [leaving, entering] = layers();
    // The old grid is the same element as before, under the new one, kept at full strength.
    expect(leaving).toBe(alphaLayer);
    expect(leaving).toHaveClass('is-leaving');
    expect(leaving).toHaveAttribute('aria-hidden', 'true');
    expect(leaving?.textContent).toContain('alpha.load');
    expect(entering).toHaveClass('is-entering');
    expect(entering?.textContent).toContain('beta.load');
    // Alpha's panels were not mounted again: their observers live on, none were made for them.
    expect(ofAlpha.every((o) => !o.disconnected)).toBe(true);
    expect(measure.observers().length - created).toBe(ofAlpha.length);
    // One frame: one header, one title, one band region, the same elements.
    expect(document.querySelectorAll('.wall-header')).toHaveLength(1);
    expect(document.querySelector('.wall')).toBe(frame);
    expect(document.querySelector('.wall-header')).toBe(header);
    expect(title()).toBe('Beta');

    await advance(400);
    expect(layers()).toHaveLength(1);
    expect(layers()[0]).toBe(entering);
    expect(layers()[0]).not.toHaveClass('is-entering');
    // The old dashboard's panels are gone, with their observers.
    expect(ofAlpha.every((o) => o.disconnected)).toBe(true);
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(4);
    expect(document.querySelector('.wall-header')).toBe(header);
  });

  it('stays idle across a dashboard change: the cursor does not come back', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    await advance(3000);
    expect(document.querySelector('.wall')).toHaveClass('is-idle');
    await walk(27_000);
    expect(frontShows()).toBe('beta');
    expect(document.querySelector('.wall')).toHaveClass('is-idle');
  });

  it('keeps at most two layers when a second change comes during a fade', async () => {
    // The state that brings an alarm arrives 200 ms after the rotation moved on to Beta.
    const late = (answer: Answer): Responder => () =>
      new Promise<Answer>((resolve) => setTimeout(() => resolve(answer), 200));
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: { json: beta },
      [dash('x')]: { json: xray },
    });
    await walk(29_000, 5000);
    mock.on(STATE, late({ json: { ...ROTATING, target: 'x' } }));
    let most = 0;
    for (let t = 0; t < 2000; t += 50) {
      await advance(50);
      await settle();
      most = Math.max(most, layers().length);
    }
    expect(frontShows()).toBe('xray');
    expect(most).toBe(2);
    await advance(400);
    expect(layers()).toHaveLength(1);
  });

  it('changes at once for reduced motion', async () => {
    const real = window.matchMedia;
    vi.spyOn(window, 'matchMedia').mockImplementation((query: string) =>
      Object.assign(real(query), { matches: query === '(prefers-reduced-motion: reduce)' }),
    );
    const ofAlpha = await rotateToBeta();
    expect(layers()).toHaveLength(1);
    expect(title()).toBe('Beta');
    expect(ofAlpha.every((o) => o.disconnected)).toBe(true);
  });
});

describe('DisplayPage: the connection', () => {
  it('keeps the picture, says after 15 s that the connection is lost and clears it on success', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(STATE, 'network');
    mock.on(dash('a'), 'network');
    // The last answer came at the start; 15 s later the picture is too old to pass as current.
    await advance(5000);
    await advance(9999);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    expect(dimmed()).toBe(false);
    await advance(1);
    expect(screen.getByText(/No connection to the server since/)).toHaveTextContent(
      'No connection to the server since 14:30',
    );
    expect(dimmed()).toBe(true);
    expect(title()).toBe('Alpha');
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(4);
    mock.on(STATE, { json: displayState() });
    mock.on(dash('a'), { json: alpha });
    await advance(5000);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    expect(dimmed()).toBe(false);
  });

  it('says the connection is lost when only the state cannot be asked for 15 s', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(STATE, 'network');
    await walk(20_000);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
    expect(dimmed()).toBe(true);
  });

  it('says that the server reported an error for answered errors', async () => {
    await open({ [STATE]: { json: displayState() }, [dash('a')]: { json: alpha } });
    mock.on(STATE, { status: 500, json: { error: 'internal', message: 'boom' } });
    await walk(20_000);
    expect(screen.getByText(/^The server reported an error since/)).toBeInTheDocument();
  });

  it('leaves no timer and no request running after unmount, and aborts the request in flight', async () => {
    const { unmount } = await open({
      [STATE]: { json: displayState() },
      [dash('a')]: [{ json: alpha }, () => new Promise(() => {})],
      'HEAD /': { headers: { ETag: '"v1"' } },
    });
    await advance(5000);
    const hanging = mock.requests.filter((r) => r.path === `/display/${ID}/dashboards/a`).at(-1);
    expect(hanging?.init.signal?.aborted).toBe(false);
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    unmount();
    await advance(100);
    expect(vi.getTimerCount()).toBe(0);
    expect(hanging?.init.signal?.aborted).toBe(true);
    const before = mock.requests.length;
    await advance(120_000);
    expect(mock.requests).toHaveLength(before);
  });
});

describe('DisplayPage: a new version', () => {
  it('reloads once when the ETag of the page changes', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      'HEAD /': [{ headers: { ETag: '"v1"' } }, { headers: { ETag: '"v2"' } }],
    });
    expect(mock.count('HEAD /')).toBe(1);
    expect(mock.requests.find((r) => r.method === 'HEAD')?.init.cache).toBe('no-store');
    await advance(59_999);
    expect(reload).not.toHaveBeenCalled();
    await advance(1);
    expect(mock.count('HEAD /')).toBe(2);
    expect(reload).toHaveBeenCalledTimes(1);
    // A server whose ETag changes on every request cannot make it reload again and again.
    await advance(120_000);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('does not reload while the ETag stays the same, or when the check fails', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      'HEAD /': [{ headers: { ETag: '"v1"' } }, 'network', { status: 503 }, { headers: { ETag: '"v1"' } }],
    });
    await advance(300_000);
    expect(mock.count('HEAD /')).toBe(6);
    expect(reload).not.toHaveBeenCalled();
  });

  it('does not reload while the connection is lost, but once the state answers again', async () => {
    await open({
      [STATE]: { json: displayState() },
      [dash('a')]: { json: alpha },
      'HEAD /': [{ headers: { ETag: '"v1"' } }, { headers: { ETag: '"v2"' } }],
    });
    mock.on(STATE, 'network');
    await walk(60_000, 5000);
    expect(mock.count('HEAD /')).toBe(2);
    expect(reload).not.toHaveBeenCalled();
    mock.on(STATE, { json: displayState() });
    await walk(60_000, 5000);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('does not reload while the token form is shown', async () => {
    await open({
      [STATE]: withToken({ json: displayState() }),
      'HEAD /': [{ headers: { ETag: '"v1"' } }, { headers: { ETag: '"v2"' } }],
    });
    expect(tokenInput()).toBeInTheDocument();
    await advance(120_000);
    expect(reload).not.toHaveBeenCalled();
  });
});
