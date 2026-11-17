import { readdirSync, readFileSync } from 'node:fs';

const css = readFileSync(`${import.meta.dirname}/../styles/base.css`, 'utf8');

function rule(selector: string): string {
  const escaped = selector.replace(/[.]/g, '\\.');
  const match = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(css);
  expect(match, `base.css has no rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

/** Every CSS file of the application, as [name, text]. */
function stylesheets(): [string, string][] {
  const root = `${import.meta.dirname}/..`;
  return ['styles', 'pages', 'components', 'shell'].flatMap((dir) =>
    readdirSync(`${root}/${dir}`, { withFileTypes: true })
      .filter((e) => !e.isDirectory() && e.name.endsWith('.css'))
      .map((e): [string, string] => [`${dir}/${e.name}`, readFileSync(`${root}/${dir}/${e.name}`, 'utf8')]),
  );
}

describe('base.css wrapping rules', () => {
  // Inside the flex row of a problem a single unbroken token must never widen its container.
  it.each(['.problem-name', '.problem-summary'])('%s breaks long unbroken text anywhere', (selector) => {
    expect(rule(selector)).toContain('overflow-wrap: anywhere');
  });

  it.each(['.problem-name', '.problem-summary'])('%s may shrink as a flex item', (selector) => {
    expect(rule(selector)).toContain('min-width: 0');
  });

  // Running text breaks only a word that cannot fit a line, so columns never shrink below a word.
  it.each(['.problem-meta', '.state-text'])('%s breaks only a word that does not fit', (selector) => {
    expect(rule(selector)).toContain('overflow-wrap: break-word');
  });

  it('uses overflow-wrap: anywhere nowhere else', () => {
    const found: string[] = [];
    for (const [name, text] of stylesheets()) {
      for (const match of text.matchAll(/([^{}]*)\{[^}]*overflow-wrap:\s*anywhere[^}]*\}/g)) {
        found.push(`${name}: ${(match[1] ?? '').trim()}`);
      }
    }
    expect(found).toEqual(['styles/base.css: .problem-name', 'styles/base.css: .problem-summary']);
  });
});
