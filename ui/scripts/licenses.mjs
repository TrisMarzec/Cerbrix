// Writes dist/licenses.txt: the licence text of every production dependency in the
// lockfile's tree. Fails on a missing licence file or a licence outside the allow-list.
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const ALLOWED = new Set(['MIT', 'ISC', 'BSD-2-Clause', 'BSD-3-Clause', 'OFL-1.1', '0BSD']);

const lock = JSON.parse(readFileSync(join(root, 'package-lock.json'), 'utf8'));
const packages = lock.packages ?? {};

// Production tree: start at the root's dependencies and follow dependencies (and
// non-optional peers that are installed), resolving like Node does, from the nearest
// node_modules directory outwards.
function resolve(from, name) {
  let base = from;
  for (;;) {
    const key = `${base}${base === '' ? '' : '/'}node_modules/${name}`;
    if (packages[key]) return key;
    if (base === '') return null;
    const idx = base.lastIndexOf('/node_modules/');
    base = idx === -1 ? '' : base.slice(0, idx);
  }
}

const seen = new Set();
const queue = Object.keys(packages[''].dependencies ?? {}).map((n) => resolve('', n));
while (queue.length > 0) {
  const key = queue.pop();
  if (key === null || key === undefined || seen.has(key)) continue;
  seen.add(key);
  const entry = packages[key];
  const deps = { ...(entry.dependencies ?? {}) };
  for (const n of Object.keys(entry.optionalDependencies ?? {})) deps[n] = true;
  for (const n of Object.keys(deps)) {
    const next = resolve(key, n);
    if (next !== null) queue.push(next);
  }
}

const errors = [];
const sections = [];
for (const key of [...seen].sort()) {
  const dir = join(root, key);
  const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
  const license = typeof manifest.license === 'string' ? manifest.license : manifest.license?.type;
  const label = `${manifest.name}@${manifest.version}`;
  if (!license || !ALLOWED.has(license)) {
    errors.push(`${label}: licence ${JSON.stringify(license)} is not allowed`);
    continue;
  }
  const file = readdirSync(dir).find((f) => /^(licen[sc]e|copying)(\.\w+)?$/i.test(f));
  if (!file) {
    errors.push(`${label}: no licence file`);
    continue;
  }
  const text = readFileSync(join(dir, file), 'utf8').trim();
  sections.push(`${label}\nLicence: ${license}\n\n${text}`);
}

if (errors.length > 0) {
  console.error(errors.join('\n'));
  process.exit(1);
}

const rule = '\n\n' + '-'.repeat(72) + '\n\n';
const header =
  'Cerbrix is licensed under the Apache License 2.0.\n' +
  'The web UI bundles the following third-party packages under their own licences.';
// The directory that receives licenses.txt is the first argument (default: dist).
const target = resolvePath(process.argv[2] ?? join(root, 'dist'));
const out = join(target, 'licenses.txt');
if (!existsSync(dirname(out))) {
  console.error(`${target} does not exist: run vite build first`);
  process.exit(1);
}
writeFileSync(out, `${header}${rule}${sections.join(rule)}\n`);
console.log(`licenses.txt: ${sections.length} packages`);
