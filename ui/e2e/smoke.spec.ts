// One pass through the web UI in a real browser, against a running server with an admin user.
//
// The state this run relies on is the one scripts/e2e.sh leaves after its API walk-through:
//   - the dashboard `e2e` ("E2E") and the display `e2e-wall` ("E2E wall") exist; the display
//     shows the dashboard and switches to it on an alarm of warning or above;
//   - the display has NO token, because the API walk-through created and revoked its token
//     (this run creates one, shows it once and starts the wall display with it; it is not
//     revoked afterwards);
//   - the host `e2e-host` has an open, unacknowledged problem "Agent silent", because the agent
//     was stopped: the wall display shows it as an alarm until this run acknowledges it, near the
//     end (so the run can run once per stack);
//   - the installation is in sync, unless CERBRIX_UI_SMOKE_EXPECT_SYNC says otherwise.
//
//   CERBRIX_UI_URL               address of the server, for example http://127.0.0.1:8443
//   CERBRIX_UI_USER              name of an admin user
//   CERBRIX_UI_PASSWORD_FILE     file that holds that user's password
//   CERBRIX_UI_SMOKE_EXPECT_SYNC in_sync (default) or out_of_sync: the state the Configuration
//                                page must show (a stack kept with CERBRIX_E2E_KEEP_STACK=1 has
//                                its repository removed and is out of sync)
//   CERBRIX_UI_SMOKE_FAIL        1 makes the first step fail on purpose; login makes the login
//                                fail on purpose after the password was typed (both show how a
//                                failure looks, and that the password does not appear in it)
//
// To run it by hand against a kept stack, see "The browser smoke run" in CONTRIBUTING.md.
//
// Neither the password nor the display token this run creates is ever printed. Neither is passed
// to Playwright's `fill`: its call log echoes the value of a failed `fill`. They are set through
// `evaluate`, whose argument Playwright does not log (checked in the pinned version's source:
// only `fill` and `type` write their text into the call log). scripts/e2e.sh masks the password
// and every token in the output as well.
import { readFileSync } from 'node:fs';

import { expect, test, type Locator, type Page } from '@playwright/test';

const HOST = 'e2e-host';
const TRIGGER = 'Agent silent';
const DASHBOARD_ID = 'e2e';
const DASHBOARD = 'E2E';
const DISPLAY_ID = 'e2e-wall';
const DISPLAY = 'E2E wall';
const COMMENT = 'ui smoke';
const FAIL_ON_PURPOSE = process.env.CERBRIX_UI_SMOKE_FAIL === '1';
const FAIL_LOGIN = process.env.CERBRIX_UI_SMOKE_FAIL === 'login';
const SYNC_STATES = { in_sync: 'In sync', out_of_sync: 'Out of sync' } as const;
const EXPECT_SYNC = process.env.CERBRIX_UI_SMOKE_EXPECT_SYNC ?? 'in_sync';

function syncText(): string {
  if (EXPECT_SYNC === 'in_sync' || EXPECT_SYNC === 'out_of_sync') return SYNC_STATES[EXPECT_SYNC];
  throw new Error('CERBRIX_UI_SMOKE_EXPECT_SYNC must be in_sync or out_of_sync');
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') throw new Error(`${name} is not set`);
  return value;
}

/** A client error that is part of the script: the browser logs every failed fetch as a console error. */
interface Allowed {
  path: string;
  status: number;
  count: number;
}

declare global {
  interface Window {
    cspViolations?: string[];
  }
}

test('the web UI works in a browser', async ({ page, baseURL, browser }) => {
  const user = required('CERBRIX_UI_USER');
  const password = readFileSync(required('CERBRIX_UI_PASSWORD_FILE'), 'utf8').replace(/\r?\n$/, '');
  if (baseURL === undefined) throw new Error('CERBRIX_UI_URL is not set');
  const origin = new URL(baseURL).origin;

  // --- Everything that must never happen, collected by listeners and checked after every step.
  const failures: string[] = [];
  // Without a session the page asks for it once per load and the server answers 401; a wrong
  // password is answered 401 as well, and so are a wall display without a token and one with a
  // wrong token. The browser logs each of them as one console error. They are allowed by path
  // and status, one per request that is expected to be refused. `clean()` fails when an
  // allowance was not used up, and an error beyond the allowances is a failure at once, so a
  // later unexpected 401 cannot hide in a leftover budget.
  let allowed: Allowed[] = [];
  const allow = (path: string, status: number, count = 1) => {
    allowed.push({ path, status, count });
  };
  const allow401 = (count = 1) => {
    allow('/api/v1/session', 401, count);
  };
  /** The path of a request, with or without a query; nothing below it. */
  const isPath = (url: string, path: string) => url === origin + path || url.startsWith(`${origin}${path}?`);

  /** Attaches the listeners to a page: the desk's, and the wall monitor's in its own context. */
  async function watch(target: Page): Promise<void> {
    target.on('console', (message) => {
      if (message.type() !== 'error') return;
      const text = message.text();
      const place = message.location().url;
      const hit = allowed.find(
        (a) => a.count > 0 && isPath(place, a.path) && text.includes(`status of ${String(a.status)}`),
      );
      if (hit !== undefined) {
        hit.count -= 1;
        return;
      }
      failures.push(`console error: ${text} (${place})`);
    });
    target.on('pageerror', (error) => {
      failures.push(`page error: ${error.message}`);
    });
    target.on('response', (response) => {
      if (response.status() >= 500) failures.push(`HTTP ${String(response.status())}: ${response.url()}`);
    });
    target.on('requestfailed', (request) => {
      if (!request.url().startsWith(origin)) return;
      const reason = request.failure()?.errorText ?? 'failed';
      // The page cancels its own polling requests when it moves on or is closed; that is not a failure.
      if (reason === 'net::ERR_ABORTED') return;
      failures.push(`request failed (${reason}): ${request.url()}`);
    });
    await target.addInitScript(() => {
      window.cspViolations = [];
      document.addEventListener('securitypolicyviolation', (event) => {
        window.cspViolations?.push(`${event.violatedDirective} blocked ${event.blockedURI}`);
      });
    });
  }
  await watch(page);

  async function clean(on: Page = page): Promise<void> {
    // A console message may arrive a moment after the step that caused it ended.
    await expect
      .poll(() => allowed.reduce((sum, a) => sum + a.count, 0), {
        message: 'allowed errors that did not happen',
      })
      .toBe(0);
    allowed = [];
    const violations = await on.evaluate(() => window.cspViolations ?? []);
    expect(violations, 'content security policy violations').toEqual([]);
    expect(failures, 'errors seen by the page listeners').toEqual([]);
  }

  async function step(title: string, body: () => Promise<void>): Promise<void> {
    await test.step(title, async () => {
      await body();
      await clean();
    });
  }

  const main = page.getByRole('main');
  const nav = page.getByRole('navigation', { name: 'Main' });
  const heading = (name: string) => page.getByRole('heading', { level: 1, name, exact: true });
  /** The dashboard's name in a wall's header. */
  const wallTitle = (on: Page) =>
    on.getByRole('banner').getByRole('heading', { level: 1, name: DASHBOARD, exact: true });

  // A secret does not go through `fill` (see the header): it is set on the field itself and the
  // page is told with an input event.
  async function typeSecret(field: Locator, secret: string): Promise<void> {
    await field.evaluate((element, value) => {
      if (!(element instanceof HTMLInputElement)) throw new Error('the secret field is not an input');
      element.value = value;
      element.dispatchEvent(new Event('input', { bubbles: true }));
    }, secret);
  }

  async function logIn(secret: string): Promise<void> {
    await page.getByLabel('User name').fill(user);
    await typeSecret(page.getByLabel('Password'), secret);
    if (FAIL_LOGIN && secret === password) {
      // Fails on purpose with the typed password in the message, to show that it is masked.
      await expect(page.getByLabel('Password')).toHaveValue('this is not the password', { timeout: 3000 });
    }
    await page.getByRole('button', { name: 'Log in' }).click();
  }

  /** Whether the page fits the window: the wall never scrolls. */
  async function expectNoScrolling(on: Page): Promise<void> {
    const sizes = await on.evaluate(() => {
      const root = document.documentElement;
      return { height: [root.scrollHeight, root.clientHeight], width: [root.scrollWidth, root.clientWidth] };
    });
    expect(sizes.height[0], 'the page scrolls vertically').toBe(sizes.height[1]);
    expect(sizes.width[0], 'the page scrolls horizontally').toBe(sizes.width[1]);
  }

  /**
   * The row of the open problem in a wall's problems panel. The rows are plain blocks without a
   * role, so they are found by the panel's class; the hidden sample row used for measuring is not one.
   */
  const wallProblem = (on: Page) =>
    on.locator('.wall-problem:not(.wall-sample)').filter({ hasText: TRIGGER }).filter({ hasText: HOST });

  await step('the login page loads without placeholder text', async () => {
    await clean();
    allow401();
    await page.goto('/');
    const expected = FAIL_ON_PURPOSE ? 'A heading that is not there' : 'Cerbrix';
    await expect(heading(expected)).toBeVisible({ timeout: FAIL_ON_PURPOSE ? 3000 : 15_000 });
    await expect(page.getByLabel('User name')).toBeVisible();
    await expect(page.getByLabel('Password')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Log in' })).toBeVisible();
    await expect(page.locator('body')).not.toContainText('was not built');
  });

  await step('a wrong password shows a message', async () => {
    allow401();
    await logIn(`wrong-${Date.now().toString(36)}`);
    // The server's own text for a failed login, shown as the page receives it.
    await expect(page.getByRole('alert')).toContainText('wrong user name or password');
    await expect(heading('Cerbrix')).toBeVisible();
  });

  await step('logging in shows the Dashboards page with the dashboard E2E', async () => {
    await logIn(password);
    await expect(heading('Dashboards')).toBeVisible();
    await expect(page).toHaveURL(/\/dashboards$/);
    await expect(
      main.getByRole('table', { name: 'Dashboards' }).getByRole('link', { name: DASHBOARD, exact: true }),
    ).toBeVisible();
  });

  await step('Hosts lists the e2e host and its page draws a graph', async () => {
    await nav.getByRole('link', { name: 'Hosts' }).click();
    await expect(heading('Hosts')).toBeVisible();
    await main.getByRole('table', { name: 'Hosts' }).getByRole('link', { name: HOST, exact: true }).click();
    await expect(heading(HOST)).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/hosts/${HOST}$`));
    await expectGraph(
      page
        .locator('section')
        .filter({ has: page.getByRole('heading', { level: 3 }) })
        .locator('canvas'),
    );
  });

  await step('reloading the host URL stays on the host page', async () => {
    // The reload asks for the session with the cookie: no 401 this time.
    await clean();
    await page.reload();
    await expect(heading(HOST)).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/hosts/${HOST}$`));
  });

  // The token of the wall display, for the step that starts it. It stays in this variable: only
  // a boolean about it reaches the report.
  let displayToken = '';

  await step('Configuration shows the sync state and a new display token in the secret dialog', async () => {
    await nav.getByRole('link', { name: 'Configuration' }).click();
    await expect(heading('Configuration')).toBeVisible();
    await expect(main.getByText(syncText(), { exact: true })).toBeVisible();
    await main.getByRole('button', { name: `Create token of ${DISPLAY}`, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: `Token of ${DISPLAY}` });
    await expect(dialog).toBeVisible();
    displayToken = await dialog.getByRole('textbox', { name: `Token of ${DISPLAY}` }).inputValue();
    expect(
      displayToken.startsWith('cbx_dt_') && displayToken.length > 'cbx_dt_'.length,
      'the dialog shows a display token',
    ).toBe(true);
    await expect(dialog).toContainText(`Open /display/${DISPLAY_ID} on the wall monitor and enter this token.`);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toBeHidden();
  });

  await step('a dashboard opened on this screen fills it and leads back to the desk', async () => {
    await nav.getByRole('link', { name: 'Dashboards' }).click();
    await expect(heading('Dashboards')).toBeVisible();
    await expect(
      main
        .getByRole('table', { name: 'Wall displays' })
        .getByRole('link', { name: `Open the wall display ${DISPLAY}`, exact: true }),
    ).toHaveAttribute('href', `/display/${DISPLAY_ID}`);
    await main
      .getByRole('table', { name: 'Dashboards' })
      .getByRole('link', { name: `Open ${DASHBOARD} on this screen`, exact: true })
      .click();
    await expect(page).toHaveURL(new RegExp(`/wall/${DASHBOARD_ID}$`));
    await expect(wallTitle(page)).toBeVisible();
    await expect(page.getByRole('navigation')).toHaveCount(0);
    const row = wallProblem(page);
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('Critical');
    await expect(row).not.toContainText('acknowledged');
    await expectGraph(page.locator('canvas'));
    await expectNoScrolling(page);
    // The link shows while the mouse moves; a move wakes the frame.
    await page.mouse.move(40, 40);
    await page.mouse.move(400, 300);
    await page.getByRole('link', { name: 'Back to the desk', exact: true }).click();
    await expect(heading('Dashboards')).toBeVisible();
    await expect(page).toHaveURL(/\/dashboards$/);
  });

  await step('a wall display without a session starts with its token and shows the alarm', async () => {
    // The monitor: a browser of its own, without the desk's session cookie.
    const monitor = await browser.newContext({ baseURL });
    const screen = await monitor.newPage();
    await watch(screen);
    const statePath = `/api/v1/display/${DISPLAY_ID}`;

    // Without a token the page asks for the display's state once and is refused.
    allow(statePath, 401);
    await screen.goto(`/display/${DISPLAY_ID}`);
    const field = screen.getByLabel('Display token');
    const startButton = screen.getByRole('button', { name: 'Start', exact: true });
    await expect(field).toBeVisible();
    await expect(startButton).toBeVisible();
    await clean(screen);

    // A wrong token is refused once more.
    allow(statePath, 401);
    await typeSecret(field, 'cbx_dt_wrong');
    await startButton.click();
    await expect(screen.getByRole('alert')).toHaveText('This token is not valid for this display.');
    await clean(screen);

    await typeSecret(field, displayToken);
    await startButton.click();
    await expect(wallTitle(screen)).toBeVisible();
    await expect(screen.getByRole('status').filter({ hasText: /^Alarm: showing/ })).toHaveText(
      new RegExp(`^Alarm: showing ${DASHBOARD}\\.`),
    );
    // The problem that caused the switch is highlighted (the panel's class for a cause).
    const row = wallProblem(screen);
    await expect(row).toHaveCount(1);
    await expect(row).toHaveClass(/(^|\s)is-cause(\s|$)/);
    await expect(row).not.toContainText('acknowledged');
    // Once it runs, the display takes no input and does not scroll.
    await expect(field).toHaveCount(0);
    const focusable = await screen.evaluate(
      () => document.querySelectorAll('a[href], button, input, select, textarea, [tabindex]').length,
    );
    expect(focusable, 'focusable elements on the running display').toBe(0);
    await expectNoScrolling(screen);
    await clean(screen);
    // Closing aborts the display's polls (5 s) and its version check (60 s); aborted requests are
    // no failure, anything else the listeners saw is checked by the next steps.
    await screen.close();
    await monitor.close();
  });

  await step('Problems lists the open problem and acknowledging it shows the user', async () => {
    await nav.getByRole('link', { name: 'Problems' }).click();
    await expect(heading('Problems')).toBeVisible();
    const row = main
      .getByRole('table', { name: 'Problems' })
      .getByRole('row')
      .filter({ hasText: TRIGGER })
      .filter({ hasText: HOST });
    await expect(row).toHaveCount(1);
    await expect(row).not.toContainText('acknowledged');
    await row.getByRole('button', { name: `Acknowledge ${TRIGGER} on ${HOST}`, exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Comment (optional)').fill(COMMENT);
    await dialog.getByRole('button', { name: 'Acknowledge', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(row).toContainText(`acknowledged by ${user}`);
    await expect(row).toContainText(COMMENT);
  });

  await step('Administration lists the admin', async () => {
    await nav.getByRole('link', { name: 'Administration' }).click();
    await expect(heading('Administration')).toBeVisible();
    await expect(
      main.getByRole('table', { name: 'Users' }).getByRole('rowheader', { name: user }),
    ).toBeVisible();
  });

  await step('logging out returns to the login page and /hosts needs a login', async () => {
    await page.getByRole('button', { name: 'Log out' }).click();
    await expect(heading('Cerbrix')).toBeVisible();
    await expect(page.getByLabel('User name')).toBeVisible();
    // A cold load of a page that needs a login: the page asks for the session once, 401.
    await clean();
    allow401();
    await page.goto('/hosts');
    await expect(heading('Cerbrix')).toBeVisible();
    await expect(page.getByLabel('Password')).toBeVisible();
    await expect(heading('Hosts')).toHaveCount(0);
  });
});

/** Waits for one of the canvases to be larger than 100 by 50 pixels: a graph that was drawn. */
async function expectGraph(canvases: Locator): Promise<void> {
  await expect
    .poll(
      async () => {
        for (const canvas of await canvases.all()) {
          const box = await canvas.boundingBox();
          if (box !== null && box.width > 100 && box.height > 50) return true;
        }
        return false;
      },
      { message: 'a graph canvas larger than 100 by 50 pixels', timeout: 30_000 },
    )
    .toBe(true);
}
