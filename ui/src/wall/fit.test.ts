import { rowsThatFit, squaresThatFit } from './fit';

describe('rowsThatFit', () => {
  it('counts the rows that fit below the label', () => {
    expect(rowsThatFit(500, 100, 50)).toBe(4);
  });

  it('counts an exact fit as a whole row (reserving a row for "and N more" is the caller\'s business)', () => {
    expect(rowsThatFit(450, 100, 50)).toBe(4);
    expect(rowsThatFit(0.3, 0.1, 0)).toBe(3);
  });

  it('is 0 when not even one row fits', () => {
    expect(rowsThatFit(140, 100, 50)).toBe(0);
    expect(rowsThatFit(40, 100, 50)).toBe(0);
  });

  it.each([
    [-500, 100, 50],
    [500, -100, 50],
    [500, 100, -50],
    [Number.NaN, 100, 50],
    [500, Number.NaN, 50],
    [500, 100, Number.NaN],
    [Number.POSITIVE_INFINITY, 100, 50],
    [500, 0, 50],
    [0, 100, 0],
  ])('is 0 for nonsense input (%s, %s, %s)', (panel, row, label) => {
    expect(rowsThatFit(panel, row, label)).toBe(0);
  });
});

describe('squaresThatFit', () => {
  it('multiplies the columns and rows that fit with their gaps', () => {
    // 10 columns: 10 * 20 + 9 * 5 = 245 ≤ 250; 4 rows: 4 * 20 + 3 * 5 = 95 ≤ 100.
    expect(squaresThatFit(250, 100, 20, 5)).toBe(40);
    // One pixel less in width: 9 columns.
    expect(squaresThatFit(244, 100, 20, 5)).toBe(36);
  });

  it('fits squares without gaps exactly', () => {
    expect(squaresThatFit(100, 50, 10, 0)).toBe(50);
  });

  it('is 0 when not even one square fits', () => {
    expect(squaresThatFit(19, 100, 20, 5)).toBe(0);
  });

  it.each([
    [0, 100, 20, 5],
    [250, 0, 20, 5],
    [250, 100, 0, 5],
    [-250, 100, 20, 5],
    [250, 100, 20, -5],
    [Number.NaN, 100, 20, 5],
    [250, Number.POSITIVE_INFINITY, 20, 5],
    [250, 100, 20, Number.NaN],
  ])('is 0 for an empty area or nonsense input (%s, %s, %s, %s)', (w, h, square, gap) => {
    expect(squaresThatFit(w, h, square, gap)).toBe(0);
  });
});
