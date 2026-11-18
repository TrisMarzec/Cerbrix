import { readFileSync } from 'node:fs';

// Without comments, so a rule's selector is all that stands before its brace.
const css = readFileSync(`${import.meta.dirname}/wall.css`, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

function rule(selector: string): string {
  const escaped = selector.replace(/[.]/g, '\\.');
  const match = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(css);
  expect(match, `wall.css has no rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

/** The wall type sizes: custom property name → N of `calc(var(--u) * N)`. */
function typeSizes(): Map<string, number> {
  const sizes = new Map<string, number>();
  for (const m of css.matchAll(/(--wall-[a-z-]+):\s*calc\(var\(--u\) \* (\d+(?:\.\d+)?)\);/g)) {
    sizes.set(m[1] ?? '', Number(m[2]));
  }
  return sizes;
}

describe('wall.css', () => {
  it('defines the scale unit as a 1920th of the viewport width on the frame', () => {
    expect(rule('.wall')).toMatch(/--u:\s*calc\(100vw \/ 1920\);/);
  });

  it('has the reference sizes of the concept as named type sizes', () => {
    expect([...typeSizes().values()]).toEqual(expect.arrayContaining([60, 48, 42, 38, 26, 24]));
  });

  it('sets every font size through a type size of at least 24 at 1920', () => {
    const sizes = typeSizes();
    const declarations = [...css.matchAll(/font-size:\s*([^;]*);/g)].map((m) => m[1] ?? '');
    expect(declarations.length).toBeGreaterThan(0);
    for (const value of declarations) {
      const name = /^var\((--wall-[a-z-]+)\)$/.exec(value)?.[1];
      expect(name, `font-size: ${value} is not a wall type size`).toBeDefined();
      const n = sizes.get(name ?? '');
      expect(n, `${name ?? value} is not defined as calc(var(--u) * N)`).toBeDefined();
      expect(n ?? 0).toBeGreaterThanOrEqual(24);
    }
  });

  it('defines no font shorthand, which would set a size past the check', () => {
    expect(css).not.toMatch(/(^|[\s;{])font:/);
  });

  it('fills the viewport exactly and never scrolls', () => {
    const frame = rule('.wall');
    expect(frame).toContain('height: 100vh');
    expect(frame).toContain('overflow: hidden');
  });

  it('measures lengths in --u, never in pixels', () => {
    expect(css).not.toMatch(/\d+px/);
  });

  it('scopes every rule under the frame', () => {
    const selectors = [...css.matchAll(/([^{}]+)\{[^{}]*\}/g)]
      .map((m) => (m[1] ?? '').trim())
      .filter((s) => !s.startsWith('@'));
    for (const list of selectors) {
      for (const selector of list.split(',')) expect(selector.trim()).toMatch(/^\.wall(?=[\s.:[>]|$)/);
    }
  });

  it('turns transitions and animations off for reduced motion', () => {
    const block = /@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?\})\s*\}/.exec(css)?.[1] ?? '';
    expect(block).toMatch(/transition:\s*none/);
    expect(block).toMatch(/animation:\s*none/);
  });

  it('hides the cursor while idle', () => {
    expect(rule('.wall.is-idle')).toContain('cursor: none');
  });

  it('dims the content while the connection is lost', () => {
    expect(rule('.wall .wall-content.is-lost')).toMatch(/opacity:\s*0\.\d+/);
  });
});
