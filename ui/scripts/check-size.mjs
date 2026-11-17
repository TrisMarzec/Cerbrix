// Fails the build when the bundle is over budget or index.html needs a policy exception.
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

// The directory to check is the first argument (default: dist next to this script's parent).
const dist = resolve(process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), '..', 'dist'));
const assets = join(dist, 'assets');
const BUDGET = { '.js': 150 * 1000, '.css': 30 * 1000 };

const totals = { '.js': 0, '.css': 0 };
for (const name of readdirSync(assets)) {
  for (const ext of Object.keys(totals)) {
    if (name.endsWith(ext)) {
      totals[ext] += gzipSync(readFileSync(join(assets, name)), { level: 9 }).length;
    }
  }
}

let failed = false;
for (const [ext, size] of Object.entries(totals)) {
  const over = size > BUDGET[ext];
  console.log(
    `${ext.slice(1).toUpperCase()}: ${(size / 1000).toFixed(1)} kB gzip of ${BUDGET[ext] / 1000} kB${over ? ' (OVER BUDGET)' : ''}`,
  );
  failed ||= over;
}

const html = readFileSync(join(dist, 'index.html'), 'utf8');
if (/<script(?![^>]*\ssrc=)[^>]*>/i.test(html)) {
  console.error(`${dist}/index.html contains an inline <script>`);
  failed = true;
}
if (/<style[\s>]/i.test(html)) {
  console.error(`${dist}/index.html contains a <style> element`);
  failed = true;
}
if (/\sstyle\s*=/i.test(html)) {
  console.error(`${dist}/index.html contains a style attribute`);
  failed = true;
}
// An inline event handler (onclick=...) is script and would need 'unsafe-inline' like a script element.
if (/<[^>]*\son[a-z]+\s*=/i.test(html)) {
  console.error(`${dist}/index.html contains an inline event-handler attribute`);
  failed = true;
}
process.exit(failed ? 1 : 0);
