import { resolveRoute } from './routes';

describe('resolveRoute', () => {
  it('keys the host page by its name so another host is a fresh component', () => {
    const a = resolveRoute('/hosts/a').element as { key: unknown };
    const a2 = resolveRoute('/hosts/a').element as { key: unknown };
    const b = resolveRoute('/hosts/b').element as { key: unknown };
    expect(a.key).toBe(a2.key);
    expect(a.key).not.toBe(b.key);
  });
});
