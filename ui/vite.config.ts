import preact from '@preact/preset-vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  base: '/',
  plugins: [preact()],
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    sourcemap: false,
    // Nothing is inlined as a data URL: the content security policy would need exceptions.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
  },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8443',
      '/healthz': 'http://127.0.0.1:8443',
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    // The browser smoke run in e2e/ needs a running server and is not part of `npm run check`.
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
  },
});
