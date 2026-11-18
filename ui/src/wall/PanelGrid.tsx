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
function placement(panel: Panel): { gridColumn: string; gridRow: string } {
  const x = clamp(panel.x, 0, COLUMNS - 1);
  const y = clamp(panel.y, 0, ROWS - 1);
  const w = clamp(panel.w, 1, COLUMNS - x);
  const h = clamp(panel.h, 1, ROWS - y);
  return { gridColumn: `${x + 1} / span ${w}`, gridRow: `${y + 1} / span ${h}` };
}

interface Props {
  panels: Panel[];
  render: (panel: Panel, index: number) => ComponentChildren;
}

/** The 12 by 6 grid of a dashboard; each panel at its cells, its content from `render`. */
export function PanelGrid({ panels, render }: Props) {
  return (
    <div class="wall-grid">
      {panels.map((panel, index) => (
        // Panels have no identity of their own; their order is fixed by the dashboard file.
        <div key={index} class="wall-panel" style={placement(panel)}>
          {render(panel, index)}
        </div>
      ))}
    </div>
  );
}
