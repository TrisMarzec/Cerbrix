// Builds the web UI so that `dist/` only ever holds a build that passed its checks.
//
// Vite builds into `dist.tmp` (git-ignored), the licence and size checks run against it, and
// only when everything passed is `dist` replaced by it and `dist/.gitkeep` re-created (the
// tracked, empty file that keeps the directory in a checkout; the server's build script
// watches it). On any failure `dist` is left as it was and `dist.tmp` is removed.
//
//   node scripts/build.mjs [--dist <dir>] [--tmp <dir>]
//
// The directories default to `dist` and `dist.tmp` next to this script's parent (ui/).
import { spawnSync } from 'node:child_process';
import { mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

function option(name, fallback) {
  const at = process.argv.indexOf(name);
  return at === -1 ? fallback : resolve(process.argv[at + 1] ?? '');
}

const dist = option('--dist', join(root, 'dist'));
const tmp = option('--tmp', join(root, 'dist.tmp'));

function run(label, script, args) {
  const done = spawnSync(process.execPath, [join(root, script), ...args], {
    cwd: root,
    stdio: 'inherit',
  });
  if (done.status !== 0) {
    throw new Error(`${label} failed${done.status === null ? '' : ` (exit ${String(done.status)})`}`);
  }
}

function remove(path) {
  rmSync(path, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

/** Replaces `dist` by `tmp` and re-creates `dist/.gitkeep`. */
export function publish(from, to) {
  remove(to);
  mkdirSync(dirname(to), { recursive: true });
  renameSync(from, to);
  writeFileSync(join(to, '.gitkeep'), '');
}

/** The part of the build that can fail; `dist` is not touched by it. */
function buildAndCheck() {
  remove(tmp);
  run('vite build', 'node_modules/vite/bin/vite.js', ['build', '--outDir', tmp, '--emptyOutDir']);
  run('licence check', 'scripts/licenses.mjs', [tmp]);
  run('size check', 'scripts/check-size.mjs', [tmp]);
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    buildAndCheck();
    publish(tmp, dist);
    console.log(`build: ${dist} replaced`);
  } catch (error) {
    remove(tmp);
    // `dist` is as it was; make sure the tracked placeholder file exists in any case.
    try {
      mkdirSync(dist, { recursive: true });
      writeFileSync(join(dist, '.gitkeep'), '');
    } catch {
      // Nothing more can be done; the original error is what matters.
    }
    console.error(`build: ${error instanceof Error ? error.message : String(error)}; ${dist} was left as it was`);
    process.exit(1);
  }
}
