import { rangeSecs, rangeWords, seriesLabel } from './series';

describe('seriesLabel', () => {
  it('is the host and the item without parameters', () => {
    expect(seriesLabel('db-01', 'cpu.usage', {})).toBe('db-01 · cpu.usage');
  });

  it('adds every parameter in key order', () => {
    expect(seriesLabel('db-01', 'disk.used_percent', { mount: '/', fs: 'ext4' })).toBe(
      'db-01 · disk.used_percent · fs=ext4 · mount=/',
    );
  });
});

describe('rangeWords', () => {
  it.each([
    ['1h', '1 hour'],
    ['6h', '6 hours'],
    ['24h', '24 hours'],
    ['7d', '7 days'],
    ['1d', '1 day'],
    ['30m', '30 minutes'],
    ['90s', '90 seconds'],
  ])('writes %s as %s', (range, words) => {
    expect(rangeWords(range)).toBe(words);
  });

  it.each([['2w'], ['1h30m'], [''], ['h']])('leaves %j as it is', (range) => {
    expect(rangeWords(range)).toBe(range);
  });
});

describe('rangeSecs', () => {
  it.each([
    ['30m', 1800],
    ['1h', 3600],
    ['6h', 21_600],
    ['24h', 86_400],
    ['7d', 604_800],
    ['90s', 90],
  ])('reads %s as %d seconds', (range, secs) => {
    expect(rangeSecs(range)).toBe(secs);
  });

  it.each([['2w'], ['1h30m'], [''], ['0h']])('has no length for %j', (range) => {
    expect(rangeSecs(range)).toBeUndefined();
  });
});
