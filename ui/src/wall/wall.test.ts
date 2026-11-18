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

/** Every rule (also inside @media and the like) as its selectors and its body. */
function rules(): { selectors: string[]; body: string }[] {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map((m) => ({ selector: (m[1] ?? '').trim(), body: m[2] ?? '' }))
    .filter(({ selector }) => !selector.startsWith('@'))
    .map(({ selector, body }) => ({ selectors: selector.split(',').map((s) => s.trim()), body }));
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
    const declarations = rules().flatMap(({ selectors, body }) =>
      [...body.matchAll(/font-size:\s*([^;]*);/g)].map((m) => ({ selectors, value: m[1] ?? '' })),
    );
    expect(declarations.length).toBeGreaterThan(0);
    for (const { selectors, value } of declarations) {
      if (value === 'inherit') {
        // Only the rule that neutralises the desk's element rules (`.wall small`, ...) inherits;
        // the frame itself declares a type size, so inheriting always ends at one.
        for (const s of selectors) expect(s, `${s} inherits its font size`).toMatch(/^\.wall [a-z0-9]+$/);
        continue;
      }
      const name = /^var\((--wall-[a-z-]+)\)$/.exec(value)?.[1];
      expect(name, `font-size: ${value} is not a wall type size`).toBeDefined();
      const n = sizes.get(name ?? '');
      expect(n, `${name ?? value} is not defined as calc(var(--u) * N)`).toBeDefined();
      expect(n ?? 0).toBeGreaterThanOrEqual(24);
    }
  });

  it('gives the frame itself a type size, so text without a wall class is never desk-sized', () => {
    const value = /font-size:\s*var\((--wall-[a-z-]+)\);/.exec(rule('.wall'))?.[1];
    expect(value).toBeDefined();
    expect(typeSizes().has(value ?? '')).toBe(true);
  });

  it('neutralises the desk element rules that would set a smaller size', () => {
    const neutral = rules().find(({ selectors }) => selectors.includes('.wall small'));
    expect(neutral?.selectors).toEqual(
      expect.arrayContaining(['.wall small', '.wall h1', '.wall h2', '.wall h3', '.wall th', '.wall td']),
    );
    expect(neutral?.body).toMatch(/font-size:\s*inherit;/);
  });

  it('defines --u and every type size exactly once, in the frame rule, in the calc form', () => {
    const names = [...css.matchAll(/(--wall-[a-z-]+|--u)\s*:/g)].map((m) => m[1] ?? '');
    expect(names).toContain('--u');
    expect(new Set(names).size, `defined more than once: ${names.join(', ')}`).toBe(names.length);
    const frame = rule('.wall');
    const inFrame = [...frame.matchAll(/(--wall-[a-z-]+|--u)\s*:/g)].map((m) => m[1] ?? '');
    expect(inFrame.sort()).toEqual([...names].sort());
    for (const m of frame.matchAll(/(--wall-[a-z-]+)\s*:\s*([^;]*);/g)) {
      expect(m[2], `${m[1] ?? ''} is not calc(var(--u) * N)`).toMatch(/^calc\(var\(--u\) \* \d+(\.\d+)?\)$/);
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

  it('measures lengths in --u; pixels only as a 1px hairline in a border', () => {
    for (const m of css.matchAll(/([a-z-]+)\s*:\s*([^;{}]*\d\s*px[^;{}]*)/g)) {
      const [, property = '', value = ''] = m;
      expect(property, `${property}: ${value} uses pixels`).toMatch(/^border(-[a-z]+)*$/);
      for (const px of value.matchAll(/[\d.]+\s*px/g)) expect(px[0], `${property}: ${value}`).toBe('1px');
    }
  });

  it('scopes every rule under the frame', () => {
    for (const { selectors } of rules()) {
      for (const selector of selectors) expect(selector).toMatch(/^\.wall(?=[\s.:[>]|$)/);
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

  it('dims the bands and the content while the connection is lost', () => {
    expect(rule('.wall .wall-body.is-lost')).toMatch(/opacity:\s*0\.\d+/);
  });

  it('hides the idle back link by opacity, so it stays in the tab order, and shows it with focus', () => {
    expect(rule('.wall.is-idle .wall-back')).toMatch(/opacity:\s*0;/);
    expect(rule('.wall.is-idle .wall-back:focus-visible')).toMatch(/opacity:\s*1;/);
    const back = rules().filter(({ selectors }) => selectors.some((s) => s.includes('.wall-back')));
    for (const { body } of back) expect(body).not.toMatch(/visibility|display:\s*none/);
  });
});
