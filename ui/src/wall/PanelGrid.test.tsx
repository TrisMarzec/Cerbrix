import { render } from '@testing-library/preact';
import { useState } from 'preact/hooks';

import type { Panel } from '../api/types';
import { graphPanelFixture, problemsPanelFixture, valuePanelFixture } from '../test/fixtures';
import { PanelGrid } from './PanelGrid';

function cells(container: Element): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>('.wall-grid > .wall-panel')];
}

describe('PanelGrid', () => {
  it('places a panel at its cells', () => {
    const { container } = render(
      <PanelGrid panels={[valuePanelFixture({ x: 8, y: 3, w: 4, h: 2 })]} render={() => <p>panel</p>} />,
    );
    const [cell] = cells(container);
    expect(cell?.style.gridColumn).toBe('9 / span 4');
    expect(cell?.style.gridRow).toBe('4 / span 2');
    expect(cell).toHaveTextContent('panel');
  });

  it('renders every one of 72 one-cell panels with its panel and index', () => {
    const panels: Panel[] = [];
    for (let y = 0; y < 6; y += 1) {
      for (let x = 0; x < 12; x += 1) panels.push(valuePanelFixture({ x, y, w: 1, h: 1 }));
    }
    const seen: [Panel, number][] = [];
    const { container } = render(
      <PanelGrid
        panels={panels}
        render={(panel, index) => {
          seen.push([panel, index]);
          return <span>{index}</span>;
        }}
      />,
    );
    const rendered = cells(container);
    expect(rendered).toHaveLength(72);
    expect(rendered[13]?.style.gridColumn).toBe('2 / span 1');
    expect(rendered[13]?.style.gridRow).toBe('2 / span 1');
    expect(rendered[13]).toHaveTextContent('13');
    expect(seen.map(([, i]) => i)).toEqual(panels.map((_, i) => i));
    expect(seen.every(([panel, i]) => panel === panels[i])).toBe(true);
  });

  it('mounts a new panel when the layout puts a panel of another type at an index, and keeps it otherwise', () => {
    let mounts = 0;
    function Probe({ label }: { label: string }) {
      useState(() => {
        mounts += 1;
        return 0;
      });
      return <span>{label}</span>;
    }
    const draw = (panel: Panel) => <Probe label={panel.type} />;
    const before = [graphPanelFixture({ x: 8, y: 3 }), valuePanelFixture({ x: 8, y: 5 })];
    const { container, rerender } = render(<PanelGrid panels={before} render={draw} />);
    expect(mounts).toBe(2);

    rerender(<PanelGrid panels={[...before]} render={draw} />);
    expect(mounts).toBe(2);

    const swapped = [valuePanelFixture({ x: 8, y: 3 }), graphPanelFixture({ x: 8, y: 5 })];
    rerender(<PanelGrid panels={swapped} render={draw} />);
    expect(mounts).toBe(4);
    expect(cells(container).map((c) => c.textContent)).toEqual(['value', 'graph']);
  });

  it('keeps panels apart that the clamp puts into the same cell', () => {
    const { container } = render(
      <PanelGrid
        panels={[valuePanelFixture({ x: 11, y: 5 }), valuePanelFixture({ x: 14, y: 9 })]}
        render={(_, index) => <span>{index}</span>}
      />,
    );
    expect(cells(container).map((c) => c.textContent)).toEqual(['0', '1']);
  });

  it('keeps a panel that leaves the grid inside it', () => {
    const { container } = render(
      <PanelGrid
        panels={[
          valuePanelFixture({ x: 11, y: 5, w: 4, h: 3 }),
          problemsPanelFixture({ x: -2, y: 9, w: 0, h: -1 }),
          valuePanelFixture({ x: 2.7, y: 1.5, w: 3.9, h: 1.2 }),
          valuePanelFixture({ x: Number.NaN, y: 0, w: Number.NaN, h: 1 }),
        ]}
        render={() => null}
      />,
    );
    const [right, wild, fractional, missing] = cells(container);
    expect(right?.style.gridColumn).toBe('12 / span 1');
    expect(right?.style.gridRow).toBe('6 / span 1');
    expect(wild?.style.gridColumn).toBe('1 / span 1');
    expect(wild?.style.gridRow).toBe('6 / span 1');
    expect(fractional?.style.gridColumn).toBe('3 / span 3');
    expect(fractional?.style.gridRow).toBe('2 / span 1');
    expect(missing?.style.gridColumn).toBe('1 / span 1');
  });
});
