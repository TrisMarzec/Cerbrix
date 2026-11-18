// One pass through the web UI in a real browser, against a running server with an admin user.
//
// The state this run relies on is the one scripts/e2e.sh leaves after its API walk-through:
//   - the dashboard `e2e` and the display `e2e-wall` ("E2E wall") exist;
//   - the display has NO token, because the API walk-through created and revoked its token
//     (this run creates one and shows it once; it is not revoked afterwards);
//   - the host `e2e-host` has an open, unacknowledged problem "Agent silent", because the agent
//     was stopped (this run acknowledges it, so it can run once per stack);
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
// Neither the password nor the display token this run creates is ever printed. The password
// is never passed to Playwright's `fill`: its call log echoes the value of a failed `fill`.
// It is set through `evaluate`, whose argument Playwright does not log (checked in the pinned
// version's source: only `fill` and `type` write their text into the call log). scripts/e2e.sh
// masks the password in the output as well.
import { readFileSync } from 'node:fs';

import { expect, test, type Page } from '@playwright/test';

const HOST = 'e2e-host';
const TRIGGER = 'Agent silent';
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

test('the web UI works in a browser', async ({ page, baseURL }) => {
  const user = required('CERBRIX_UI_USER');
  const password = readFileSync(required('CERBRIX_UI_PASSWORD_FILE'), 'utf8').replace(/\r?\n$/, '');
  if (baseURL === undefined) throw new Error('CERBRIX_UI_URL is not set');
  const origin = new URL(baseURL).origin;

  // --- Everything that must never happen, collected by listeners and checked after every step.
  const failures: string[] = [];
  // Without a session the page asks for it once per load and the server answers 401; a wrong
  // password is answered 401 as well. The browser logs each of them as one console error. They
  // are allowed by URL and status, one per navigation or attempt that is expected to be logged
  // out. `clean()` fails when an allowance was not used up, and an error beyond the allowances
  // is a failure at once, so a later unexpected 401 cannot hide in a leftover budget.
  let allowed: Allowed[] = [];
  const allow401 = (count = 1) => {
    allowed.push({ path: '/api/v1/session', status: 401, count });
  };

  page.on('console', (message) => {
    if (message.type() !== 'error') return;
    const text = message.text();
    const place = message.location().url;
    const hit = allowed.find(
      (a) => a.count > 0 && place.startsWith(origin + a.path) && text.includes(`status of ${String(a.status)}`),
    );
    if (hit !== undefined) {
      hit.count -= 1;
      return;
    }
    failures.push(`console error: ${text} (${place})`);
  });
  page.on('pageerror', (error) => {
    failures.push(`page error: ${error.message}`);
  });
  page.on('response', (response) => {
    if (response.status() >= 500) failures.push(`HTTP ${String(response.status())}: ${response.url()}`);
  });
  page.on('requestfailed', (request) => {
    if (!request.url().startsWith(origin)) return;
    const reason = request.failure()?.errorText ?? 'failed';
    // The page cancels its own polling requests when it moves on; that is not a failure.
    if (reason === 'net::ERR_ABORTED') return;
    failures.push(`request failed (${reason}): ${request.url()}`);
  });
  await page.addInitScript(() => {
    window.cspViolations = [];
    document.addEventListener('securitypolicyviolation', (event) => {
      window.cspViolations?.push(`${event.violatedDirective} blocked ${event.blockedURI}`);
    });
  });

  async function clean(): Promise<void> {
    // A console message may arrive a moment after the step that caused it ended.
    await expect
      .poll(() => allowed.reduce((sum, a) => sum + a.count, 0), {
        message: 'allowed errors that did not happen',
      })
      .toBe(0);
    allowed = [];
    const violations = await page.evaluate(() => window.cspViolations ?? []);
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

  // The password does not go through `fill` (see the header): it is set on the field itself and
  // the page is told with an input event.
  async function typePassword(secret: string): Promise<void> {
    await page.getByLabel('Password').evaluate((field, value) => {
      if (!(field instanceof HTMLInputElement)) throw new Error('the password field is not an input');
      field.value = value;
      field.dispatchEvent(new Event('input', { bubbles: true }));
    }, secret);
  }

  async function logIn(secret: string): Promise<void> {
    await page.getByLabel('User name').fill(user);
    await typePassword(secret);
    if (FAIL_LOGIN && secret === password) {
      // Fails on purpose with the typed password in the message, to show that it is masked.
      await expect(page.getByLabel('Password')).toHaveValue('this is not the password', { timeout: 3000 });
    }
    await page.getByRole('button', { name: 'Log in' }).click();
  }

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
      main.getByRole('table', { name: 'Dashboards' }).getByRole('link', { name: 'E2E', exact: true }),
    ).toBeVisible();
  });

  await step('Hosts lists the e2e host and its page draws a graph', async () => {
    await nav.getByRole('link', { name: 'Hosts' }).click();
    await expect(heading('Hosts')).toBeVisible();
    await main.getByRole('table', { name: 'Hosts' }).getByRole('link', { name: HOST, exact: true }).click();
    await expect(heading(HOST)).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/hosts/${HOST}$`));
    await expectGraph(page);
  });

  await step('reloading the host URL stays on the host page', async () => {
    // The reload asks for the session with the cookie: no 401 this time.
    await clean();
    await page.reload();
    await expect(heading(HOST)).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/hosts/${HOST}$`));
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

  await step('Configuration shows the sync state and a new display token in the secret dialog', async () => {
    await nav.getByRole('link', { name: 'Configuration' }).click();
    await expect(heading('Configuration')).toBeVisible();
    await expect(main.getByText(syncText(), { exact: true })).toBeVisible();
    await main.getByRole('button', { name: `Create token of ${DISPLAY}`, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: `Token of ${DISPLAY}` });
    await expect(dialog).toBeVisible();
    // The value stays in this local variable: only a boolean reaches the report.
    const secret = await dialog.getByRole('textbox', { name: `Token of ${DISPLAY}` }).inputValue();
    expect(
      secret.startsWith('cbx_dt_') && secret.length > 'cbx_dt_'.length,
      'the dialog shows a display token',
    ).toBe(true);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toBeHidden();
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

/** Waits for a canvas in a graph frame (a section with a level 3 heading) larger than 100 by 50 pixels. */
async function expectGraph(page: Page): Promise<void> {
  const canvases = page
    .locator('section')
    .filter({ has: page.getByRole('heading', { level: 3 }) })
    .locator('canvas');
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
