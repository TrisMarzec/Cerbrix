import { defineConfig, devices } from '@playwright/test';

const baseURL = process.env.CERBRIX_UI_URL;
// A server reached over plain HTTP under a name other than localhost (the end-to-end stack, by its
// service name) is not a secure context for the browser, which then logs an error for the server's
// own security headers. Production runs on HTTPS: treat this one origin as secure, as it would be.
const url = baseURL === undefined ? undefined : new URL(baseURL);
const plainOrigin =
  url !== undefined && url.protocol === 'http:' && !['localhost', '127.0.0.1'].includes(url.hostname)
    ? url.origin
    : undefined;

// The browser smoke run against a running Cerbrix server (see CONTRIBUTING.md). It starts no
// server of its own. Traces, screenshots and videos stay off: they could contain a secret.
export default defineConfig({
  testDir: 'e2e',
  testMatch: 'smoke.spec.ts',
  retries: 0,
  workers: 1,
  reporter: 'list',
  timeout: 120_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: {
          args: plainOrigin === undefined ? [] : [`--unsafely-treat-insecure-origin-as-secure=${plainOrigin}`],
        },
      },
    },
  ],
});
