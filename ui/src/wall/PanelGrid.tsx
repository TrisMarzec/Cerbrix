import type { ComponentChildren } from 'preact';

import type { Panel } from '../api/types';

const COLUMNS = 12;
const ROWS = 6;

/** A whole number between `min` and `max`; anything that is not a number becomes `min`. */
function clamp(n: number, min: number, max: number): number {
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, Math.floor(n)));
}

/**
 * The CSS grid lines of a panel. The configuration compiler rejects panels that leave the grid,
 * so this only keeps a bad value from breaking the layout.
 */
function placement(panel: Panel): { x: number; y: number; gridColumn: string; gridRow: string } {
  const x = clamp(panel.x, 0, COLUMNS - 1);
  const y = clamp(panel.y, 0, ROWS - 1);
  const w = clamp(panel.w, 1, COLUMNS - x);
  const h = clamp(panel.h, 1, ROWS - y);
  return { x, y, gridColumn: `${x + 1} / span ${w}`, gridRow: `${y + 1} / span ${h}` };
}

/**
 * Keys by type and cell, so a changed layout mounts a new panel instead of handing a graph the
 * props of another panel. Two panels can only share a cell through the clamp; they get a suffix.
 */
function keyed(panels: Panel[]) {
  const seen = new Map<string, number>();
  return panels.map((panel, index) => {
    const { x, y, gridColumn, gridRow } = placement(panel);
    const base = `${panel.type}:${x}:${y}`;
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return { panel, index, key: n === 0 ? base : `${base}#${n}`, style: { gridColumn, gridRow } };
  });
}

interface Props {
  panels: Panel[];
  render: (panel: Panel, index: number) => ComponentChildren;
}

/** The 12 by 6 grid of a dashboard; each panel at its cells, its content from `render`. */
export function PanelGrid({ panels, render }: Props) {
  return (
    <div class="wall-grid">
      {keyed(panels).map(({ panel, index, key, style }) => (
        <div key={key} class="wall-panel" style={style}>
          {render(panel, index)}
        </div>
      ))}
    </div>
  );
}
