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
const alpha = dashboardDetailFixture({ id: 'a', name: 'Alpha' });
const beta = dashboardDetailFixture({ id: 'b', name: 'Beta' });
const gamma = dashboardDetailFixture({ id: 'c', name: 'Gamma' });

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

/** The title of the dashboard in front (not the one fading out). */
function title(): string {
  return document.querySelector('.wall-layer:not(.is-leaving) .wall-title')?.textContent ?? '';
}

function layers(): Element[] {
  return [...document.querySelectorAll('.wall-layer')];
}

function dimmed(): boolean {
  return document.querySelector('.wall-layer:not(.is-leaving) .wall-body')?.classList.contains('is-lost') === true;
}

function band(): string | null {
  return document.querySelector('.wall-layer:not(.is-leaving) .wall-band.is-alarm')?.textContent ?? null;
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
    expect(input).toHaveAttribute('type', 'password');
    expect(input).toHaveAttribute('autocomplete', 'off');
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

  it('keeps the old dashboard until the new one has answered', async () => {
    await open({
      [STATE]: { json: ROTATING },
      [dash('a')]: { json: alpha },
      [dash('b')]: () => new Promise(() => {}),
    });
    await advance(30_000);
    expect(mock.count(dash('b'))).toBe(1);
    expect(title()).toBe('Alpha');
    expect(layers()).toHaveLength(1);
  });

  it('says after 15 s that the connection is lost when the new dashboard cannot be loaded, the old one dimmed', async () => {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: 'network' });
    await advance(30_000);
    await settle();
    expect(title()).toBe('Alpha');
    await advance(14_999);
    expect(screen.queryByText(/No connection to the server/)).not.toBeInTheDocument();
    await advance(1);
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
    expect(title()).toBe('Alpha');
    expect(dimmed()).toBe(true);
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

describe('DisplayPage: the dashboard change', () => {
  async function rotateToBeta() {
    await open({ [STATE]: { json: ROTATING }, [dash('a')]: { json: alpha }, [dash('b')]: { json: beta } });
    const ofAlpha = measure.observers().filter((o) => !o.disconnected);
    expect(ofAlpha.length).toBeGreaterThan(0);
    await advance(30_000);
    await settle();
    return ofAlpha;
  }

  it('fades the old dashboard out and the new one in over 400 ms, then keeps only the new one', async () => {
    const ofAlpha = await rotateToBeta();
    expect(layers()).toHaveLength(2);
    const [leaving, entering] = layers();
    expect(leaving).toHaveClass('is-leaving');
    expect(leaving).toHaveAttribute('aria-hidden', 'true');
    expect(leaving?.querySelector('.wall-title')).toHaveTextContent('Alpha');
    expect(entering).toHaveClass('is-entering');
    expect(entering?.querySelector('.wall-title')).toHaveTextContent('Beta');
    await advance(400);
    expect(layers()).toHaveLength(1);
    expect(layers()[0]).not.toHaveClass('is-entering');
    expect(title()).toBe('Beta');
    // The old dashboard's panels are gone, with their observers.
    expect(ofAlpha.every((o) => o.disconnected)).toBe(true);
    expect(document.querySelectorAll('.wall-panel')).toHaveLength(4);
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
    await advance(5000);
    await advance(14_999);
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
