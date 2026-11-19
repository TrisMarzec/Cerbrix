import { readdirSync, readFileSync } from 'node:fs';

const root = `${import.meta.dirname}/..`;

function stylesheets(): [string, string][] {
  return ['styles', 'pages', 'components', 'shell'].flatMap((dir) =>
    readdirSync(`${root}/${dir}`, { withFileTypes: true })
      .filter((e) => !e.isDirectory() && e.name.endsWith('.css'))
      .map((e): [string, string] => [
        `${dir}/${e.name}`,
        // Without comments, so a rule's selector is all that stands before its brace.
        readFileSync(`${root}/${dir}/${e.name}`, 'utf8').replace(/\/\*[\s\S]*?\*\//g, ''),
      ]),
  );
}

describe('acknowledged at half strength', () => {
  it('has one rule in base.css that mixes the severity colour to half strength', () => {
    const base = readFileSync(`${root}/styles/base.css`, 'utf8');
    // The mix only where the browser has color-mix; elsewhere the full colour (fallbacks.test.ts).
    const rule = /\n@supports \(color: color-mix\([^)]*\)\) \{\s*\.is-acknowledged\s*\{([^}]*)\}/.exec(base)?.[1] ?? '';
    expect(rule).toMatch(/--sev:\s*color-mix\(in srgb, var\(--sev-full\) 50%, transparent\)/);
    const fallback = /\n\.is-acknowledged\s*\{([^}]*)\}/.exec(base)?.[1] ?? '';
    expect(fallback).toMatch(/--sev:\s*var\(--sev-full\);/);
    // Every severity class gives the full colour the rule mixes.
    for (const s of ['critical', 'high', 'warning', 'info']) {
      expect(base).toMatch(new RegExp(`\\.sev-${s}\\s*\\{[^}]*--sev-full:`));
    }
  });

  it('is the only rule about acknowledged things that changes their colour or opacity', () => {
    const found: string[] = [];
    for (const [name, text] of stylesheets()) {
      for (const match of text.matchAll(/([^{}]*\.is-acknowledged[^{}]*)\{([^}]*)\}/g)) {
        const selector = (match[1] ?? '').trim();
        if (/opacity|--sev/.test(match[2] ?? '')) found.push(`${name}: ${selector}`);
      }
    }
    // The rule and its fallback for browsers without color-mix.
    expect(found).toEqual(['styles/base.css: .is-acknowledged', 'styles/base.css: .is-acknowledged']);
  });

  it('comes after the severity classes, so it wins on the same element', () => {
    const base = readFileSync(`${root}/styles/base.css`, 'utf8');
    expect(base.indexOf('\n.is-acknowledged {')).toBeGreaterThan(base.indexOf('\n.sev-info {'));
  });
});
