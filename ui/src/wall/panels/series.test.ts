import { rangeWords, seriesLabel } from './series';

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
