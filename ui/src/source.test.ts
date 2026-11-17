import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const root = import.meta.dirname;

function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? files(join(dir, e.name)) : [join(dir, e.name)],
  );
}

const all = files(root).map((path) => ({
  rel: relative(root, path).split(sep).join('/'),
  text: readFileSync(path, 'utf8'),
}));

// The forbidden words are assembled so this file does not contain them itself.
const forbidden = ['dangerously' + 'SetInnerHTML', 'inner' + 'HTML', 'eval' + '('];
const colour = /#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b|\b(?:rgba?|hsla?)\(/;

describe('source rules', () => {
  it('finds the source files', () => {
    expect(all.length).toBeGreaterThan(5);
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
