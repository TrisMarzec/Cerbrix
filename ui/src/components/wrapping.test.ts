import { readFileSync } from 'node:fs';

const css = readFileSync(`${import.meta.dirname}/../styles/base.css`, 'utf8');

function rule(selector: string): string {
  const escaped = selector.replace(/[.]/g, '\\.');
  const match = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(css);
  expect(match, `base.css has no rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

describe('base.css wrapping rules', () => {
  it.each(['.problem-name', '.problem-summary', '.problem-meta', '.state-text'])(
    '%s wraps long unbroken text',
    (selector) => {
      expect(rule(selector)).toContain('overflow-wrap: anywhere');
    },
  );

  it.each(['.problem-name', '.problem-summary'])('%s may shrink as a flex item', (selector) => {
    expect(rule(selector)).toContain('min-width: 0');
  });
});
