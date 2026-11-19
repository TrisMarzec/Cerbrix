import { readdirSync, readFileSync } from 'node:fs';

/*
 * Older browsers (older Chromium, Firefox ESR) lack `color-mix` and `@starting-style`. Without a
 * fallback, a colour set with `color-mix` vanishes there entirely (the acknowledged colour, the
 * cause highlight), so every such declaration has one that such a browser takes instead.
 */

const root = `${import.meta.dirname}/..`;

function stylesheets(): [string, string][] {
  return ['styles', 'pages', 'components', 'shell', 'wall'].flatMap((dir) =>
    readdirSync(`${root}/${dir}`, { withFileTypes: true })
      .filter((e) => !e.isDirectory() && e.name.endsWith('.css'))
      .map((e): [string, string] => [
        `${dir}/${e.name}`,
        readFileSync(`${root}/${dir}/${e.name}`, 'utf8').replace(/\/\*[\s\S]*?\*\//g, ''),
      ]),
  );
}

interface Rule {
  selector: string;
  /** The at-rules the rule stands in, outermost first, such as `@supports (...)`. */
  within: string[];
  declarations: { property: string; value: string }[];
}

/** Every style rule of a stylesheet, also those inside at-rules. */
function rulesOf(css: string): Rule[] {
  const rules: Rule[] = [];
  const stack: string[] = [];
  let prelude = '';
  for (const char of css) {
    if (char === '{') {
      stack.push(prelude.trim());
      prelude = '';
    } else if (char === '}') {
      const selector = stack.pop() ?? '';
      if (!selector.startsWith('@')) {
        const declarations = prelude
          .split(';')
          .map((d) => d.trim())
          .filter((d) => d.includes(':'))
          .map((d) => {
            const at = d.indexOf(':');
            return { property: d.slice(0, at).trim(), value: d.slice(at + 1).trim() };
          });
        rules.push({ selector, within: [...stack], declarations });
      }
      prelude = '';
    } else {
      prelude += char;
    }
  }
  return rules;
}

const all = stylesheets().flatMap(([name, css]) => rulesOf(css).map((rule) => ({ name, rule })));

describe('fallbacks for older browsers', () => {
  it('finds the rules that use color-mix', () => {
    const using = all.filter(({ rule }) => rule.declarations.some((d) => d.value.includes('color-mix(')));
    expect(using.map(({ name, rule }) => `${name}: ${rule.selector}`)).toEqual(
      expect.arrayContaining([
        'styles/base.css: .is-acknowledged',
        'styles/base.css: .dialog-backdrop',
        'wall/wall.css: .wall .wall-problem.is-cause',
      ]),
    );
  });

  it('gives every color-mix declaration a fallback of the same property before it', () => {
    for (const { name, rule } of all) {
      rule.declarations.forEach((d, i) => {
        if (!d.value.includes('color-mix(')) return;
        const where = `${name}: ${rule.selector} { ${d.property} }`;
        if (d.property.startsWith('--')) {
          // A custom property takes any value, so the last declaration always wins and a browser
          // without color-mix makes the colour invalid where it is used. Its fallback is the same
          // property in the same selector outside an @supports block that asks for color-mix.
          expect(rule.within.some((at) => /^@supports\s*\(.*color-mix\(/.test(at)), where).toBe(true);
          const outside = all.filter(
            (other) =>
              other.name === name &&
              other.rule.selector === rule.selector &&
              !other.rule.within.some((at) => at.startsWith('@supports')) &&
              other.rule.declarations.some((o) => o.property === d.property && !o.value.includes('color-mix(')),
          );
          expect(outside, `${where} has no fallback outside @supports`).not.toHaveLength(0);
          return;
        }
        const before = rule.declarations
          .slice(0, i)
          .filter((o) => o.property === d.property && !o.value.includes('color-mix('));
        expect(before, `${where} has no fallback before it`).not.toHaveLength(0);
      });
    }
  });

  it('uses @starting-style only for where a fade starts, never for how a thing looks once shown', () => {
    for (const { name, rule } of all) {
      if (!rule.within.includes('@starting-style')) continue;
      for (const d of rule.declarations) {
        expect(['opacity', 'background-color'], `${name}: ${rule.selector} { ${d.property} }`).toContain(d.property);
      }
      // The rule it starts from exists outside @starting-style, with a transition of that property.
      const shown = all.find(
        (other) =>
          other.name === name &&
          other.rule.selector === rule.selector &&
          !other.rule.within.includes('@starting-style'),
      );
      expect(shown?.rule.declarations.some((d) => d.property === 'transition'), rule.selector).toBe(true);
    }
  });
});
