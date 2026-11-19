import { durationParts, durationSecs } from './duration';

describe('durationSecs', () => {
  it.each([
    ['30s', 30],
    ['5m', 300],
    ['1h', 3600],
    ['1m30s', 90],
    ['90s', 90],
    ['1d', 86_400],
    ['0s', 0],
    ['', undefined],
    ['30', undefined],
    ['s', undefined],
    ['5 min', undefined],
    ['1.5m', undefined],
    ['-5s', undefined],
    ['30S', undefined],
    ['often', undefined],
  ])('reads %j as %j seconds', (text, seconds) => {
    expect(durationSecs(text)).toBe(seconds);
  });

  it('gives the parts as written, for words', () => {
    expect(durationParts('1m30s')).toEqual([
      { amount: 1, unit: 'm' },
      { amount: 30, unit: 's' },
    ]);
    expect(durationParts('90s')).toEqual([{ amount: 90, unit: 's' }]);
    expect(durationParts('soon')).toBeUndefined();
  });
});
