/** Rounding noise of measured sizes, so that an exact fit is not lost to floating point. */
const EPSILON = 1e-9;

function usable(...sizes: number[]): boolean {
  return sizes.every((s) => Number.isFinite(s) && s >= 0);
}

/**
 * How many rows of `rowHeightPx` fit into a panel of `panelHeightPx` below its label of
 * `labelHeightPx`; 0 for non-finite, negative or zero sizes.
 *
 * This counts plain rows. When not all items fit, the caller reserves one of these rows for
 * "and N more": with room for 4 rows and 10 items it shows 3 rows and "and 7 more".
 */
export function rowsThatFit(panelHeightPx: number, rowHeightPx: number, labelHeightPx: number): number {
  if (!usable(panelHeightPx, rowHeightPx, labelHeightPx) || panelHeightPx <= 0 || rowHeightPx <= 0) return 0;
  return Math.max(0, Math.floor((panelHeightPx - labelHeightPx) / rowHeightPx + EPSILON));
}

/** How many squares fit along a length, with a gap between neighbours (none after the last). */
function alongLength(lengthPx: number, squarePx: number, gapPx: number): number {
  return Math.max(0, Math.floor((lengthPx + gapPx) / (squarePx + gapPx) + EPSILON));
}

/**
 * How many squares of `squarePx` with `gapPx` between them fit into an area of `widthPx` by
 * `heightPx`: columns times rows; 0 for non-finite, negative or zero sizes.
 */
export function squaresThatFit(widthPx: number, heightPx: number, squarePx: number, gapPx: number): number {
  if (!usable(widthPx, heightPx, squarePx, gapPx) || widthPx <= 0 || heightPx <= 0 || squarePx <= 0) return 0;
  return alongLength(widthPx, squarePx, gapPx) * alongLength(heightPx, squarePx, gapPx);
}
