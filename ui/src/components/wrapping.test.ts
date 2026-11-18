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

  it('uses overflow-wrap: anywhere only for problem text and the monospace trigger expression', () => {
    const found: string[] = [];
    for (const [name, text] of stylesheets()) {
      for (const match of text.matchAll(/([^{}]*)\{[^}]*overflow-wrap:\s*anywhere[^}]*\}/g)) {
        const selector = (match[1] ?? '').replace(/\/\*[^]*?\*\//g, '').trim();
        found.push(`${name}: ${selector}`);
      }
    }
    expect(found).toEqual([
      'styles/base.css: .problem-name',
      'styles/base.css: .problem-summary',
      'pages/HostPage.css: .host-expression',
    ]);
  });
});

describe('page width', () => {
  it('scrolls a table inside its wrapper, not the page', () => {
    const body = rule('.table-scroll');
    expect(body).toContain('overflow-x: auto');
    expect(body).toContain('min-width: 0');
  });

  it('gives every track of a grid with two columns a zero minimum (minmax(0, ...))', () => {
    const hostCss = readFileSync(`${import.meta.dirname}/../pages/HostPage.css`, 'utf8');
    const columns = /\.host-columns\s*\{[^}]*grid-template-columns:\s*([^;]*);/.exec(hostCss)?.[1] ?? '';
    expect(columns.match(/minmax\(0,/g)).toHaveLength(2);
    const stacked = /@media[^{]*\{\s*\.host-columns\s*\{[^}]*grid-template-columns:\s*([^;]*);/.exec(hostCss)?.[1] ?? '';
    expect(stacked).toContain('minmax(0,');
  });

  it('lets the column wrappers shrink', () => {
    const hostCss = readFileSync(`${import.meta.dirname}/../pages/HostPage.css`, 'utf8');
    expect(/\.host-left,\s*\.host-right\s*\{[^}]*min-width:\s*0/.exec(hostCss)).not.toBeNull();
  });
});
