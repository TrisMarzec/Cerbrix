import { readdirSync, readFileSync } from 'node:fs';

/** The paths of all files below `dir`, relative to it, with forward slashes. */
function files(dir: string, prefix = ''): string[] {
  return readdirSync(`${dir}/${prefix}`, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? files(dir, `${prefix}${e.name}/`) : [`${prefix}${e.name}`],
  );
}

const root = import.meta.dirname;
const all = files(root).map((rel) => ({ rel, text: readFileSync(`${root}/${rel}`, 'utf8') }));

// The forbidden words are assembled so this file does not contain them itself.
const forbidden = ['dangerously' + 'SetInnerHTML', 'inner' + 'HTML', 'eval' + '('];
const colour = /#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b|\b(?:rgba?|hsla?)\(/;

describe('source rules', () => {
  it('finds the source files and reads them', () => {
    expect(all.length).toBeGreaterThan(5);
    const tokens = all.find((f) => f.rel === 'styles/tokens.css');
    expect(tokens?.text).toMatch(colour);
  });

  it('has no inner-HTML or eval use anywhere', () => {
    for (const f of all) {
      for (const word of forbidden) {
        expect(f.text.includes(word), `${f.rel} contains ${word}`).toBe(false);
      }
    }
  });

  it('has colour literals only in tokens.css', () => {
    for (const f of all) {
      if (/\.test\.tsx?$/.test(f.rel) || f.rel.startsWith('test/') || f.rel === 'styles/tokens.css') {
        continue;
      }
      expect(colour.test(f.text), `${f.rel} contains a colour literal`).toBe(false);
    }
  });
});
