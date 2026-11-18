import { act, render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';
import { vi } from 'vitest';

import type { GraphPanelData } from '../../api/types';
import { NOW, graphPanelFixture } from '../../test/fixtures';
import { installMeasure, releaseMeasure, type Measure } from '../test/measure';
import { GraphPanel } from './GraphPanel';

interface Instance {
  opts: uPlot.Options;
  data: uPlot.AlignedData;
  setSize: ReturnType<typeof vi.fn>;
  setData: ReturnType<typeof vi.fn>;
  destroy: ReturnType<typeof vi.fn>;
}

const instances: Instance[] = [];

vi.mock('uplot', () => ({
  default: class {
    constructor(opts: uPlot.Options, data: uPlot.AlignedData) {
      const self = this as unknown as Instance;
      self.opts = opts;
      self.data = data;
      self.setSize = vi.fn();
      self.setData = vi.fn();
      self.destroy = vi.fn();
      instances.push(self);
    }
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

const innerWidth = Object.getOwnPropertyDescriptor(window, 'innerWidth');

/** What the test helper does not measure: the plot's container and the "No data" sample. */
let extra = { canvasWidth: 0, canvasHeight: 0, noDataHeight: 0 };
let measure: Measure;

/** A panel body of `height` pixels at 1920 (u = 1, so the axis font is 24 and a plot needs 96). */
function layout(height: number) {
  measure = installMeasure({ bodyWidth: 600, bodyHeight: height });
  extra = { canvasWidth: 600, canvasHeight: height, noDataHeight: 46 };
}

beforeEach(() => {
  instances.length = 0;
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1920 });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('graph-canvas')) return new DOMRect(0, 0, extra.canvasWidth, extra.canvasHeight);
    if (this.classList.contains('wall-nodata') && this.classList.contains('wall-sample')) {
      return new DOMRect(0, 0, 200, extra.noDataHeight);
    }
    // The body and the helper's samples: the box installMeasure gives them.
    return Element.prototype.getBoundingClientRect.call(this);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  releaseMeasure();
  if (innerWidth !== undefined) Object.defineProperty(window, 'innerWidth', innerWidth);
});

function show(overrides: Partial<GraphPanelData> = {}) {
  return render(<GraphPanel panel={graphPanelFixture(overrides)} now={NOW} />);
}

function last(): Instance {
  const instance = instances.at(-1);
  if (instance === undefined) throw new Error('uPlot was not created');
  return instance;
}

function labelText(container: Element): string {
  return container.querySelector('.wall-panel-label-text')?.textContent ?? '';
}

function latest(container: Element): string | null {
  return container.querySelector('.wall-panel-latest')?.textContent ?? null;
}

/** The shown "No data", without the hidden sample. */
function noData(container: Element): Element | null {
  return container.querySelector('.wall-nodata:not(.wall-sample)');
}

describe('GraphPanel', () => {
  it('is labelled with the host and the item', () => {
    layout(200);
    const { container } = show();
    expect(labelText(container)).toBe('db-01 · cpu.usage');
  });

  it('adds the parameters to the label in key order', () => {
    layout(200);
    const { container } = show({ item: 'disk.used_percent', params: { mount: '/', fs: 'ext4' } });
    expect(labelText(container)).toBe('db-01 · disk.used_percent · fs=ext4 · mount=/');
  });

  it('shows the value of the last point at the right of the label', () => {
    layout(200);
    const { container } = show();
    expect(latest(container)).toBe('27.4 %');
  });

  it('draws the points as a wall graph that fills the body, without interaction', () => {
    layout(200);
    show();
    expect(instances).toHaveLength(1);
    const { opts, data } = last();
    expect(opts).toMatchObject({ width: 600, height: 200 });
    expect(opts.cursor?.show).toBe(false);
    expect(opts.axes?.[0]?.font).toMatch(/^24px /);
    expect(data[1]).toEqual([28.2028656006, 27.432800292968]);
    expect(screen.queryByTestId('graph-readout')).toBeNull();
  });

  it('describes the graph with the host, the range, the latest value and the thresholds', () => {
    layout(200);
    show({ range: '6h' });
    expect(
      screen.getByRole('img', { name: 'cpu.usage of db-01 over the last 6 hours, latest 27.4 %, 1 threshold' }),
    ).toBeInTheDocument();
  });

  it('passes the thresholds to the graph', () => {
    layout(200);
    show({
      thresholds: [
        { trigger: 'CPU high', severity: 'high', op: '>', value: 90 },
        { trigger: 'CPU busy', severity: 'warning', op: '>', value: 70 },
      ],
    });
    const range = last().opts.scales?.['y']?.range as (u: unknown, min: number, max: number) => [number, number];
    expect(range({}, 27, 28)[1]).toBeGreaterThanOrEqual(90);
    expect(screen.getByRole('img').getAttribute('aria-label')).toMatch(/, 2 thresholds$/);
  });

  it.each([[null], [[]]])('says "No data" for points %j, without a plot or a latest value', (points) => {
    layout(200);
    const { container } = show({ points });
    expect(noData(container)).toHaveTextContent('No data');
    expect(latest(container)).toBeNull();
    expect(instances).toHaveLength(0);
  });

  it('shows only the label, with the latest value, when the body is too low for a plot', () => {
    layout(80);
    const { container } = show();
    expect(instances).toHaveLength(0);
    expect(container.querySelector('.graph')).toBeNull();
    expect(latest(container)).toBe('27.4 %');
  });

  it('shows only the label when the body is too low for "No data"', () => {
    layout(40);
    const { container } = show({ points: null });
    expect(noData(container)).toBeNull();
    expect(labelText(container)).toBe('db-01 · cpu.usage');
  });

  it('draws once it has room after a resize', () => {
    layout(80);
    show();
    expect(instances).toHaveLength(0);
    measure.set({ bodyHeight: 200 });
    extra.canvasHeight = 200;
    measure.resize();
    expect(instances).toHaveLength(1);
  });

  it('destroys the plot on unmount', () => {
    layout(200);
    const view = show();
    act(() => {
      view.unmount();
    });
    expect(last().destroy).toHaveBeenCalledTimes(1);
  });

  it.each<[string, Partial<GraphPanelData>]>([
    ['host', { host: 'db-02' }],
    ['item', { item: 'load.1m' }],
    ['params', { params: { mount: '/' } }],
    ['range', { range: '24h' }],
  ])('creates a new plot when the %s changes', (_what, change) => {
    layout(200);
    const view = show();
    const first = last();
    view.rerender(<GraphPanel panel={graphPanelFixture(change)} now={NOW} />);
    expect(first.destroy).toHaveBeenCalledTimes(1);
    expect(instances).toHaveLength(2);
  });

  it('feeds new points of the same series into the same plot', () => {
    layout(200);
    const view = show();
    const next: [number, number][] = [
      [NOW.getTime(), 27.4],
      [NOW.getTime() + 15_000, 31],
    ];
    view.rerender(<GraphPanel panel={graphPanelFixture({ points: next })} now={new Date(NOW.getTime() + 15_000)} />);
    expect(instances).toHaveLength(1);
    expect(last().destroy).not.toHaveBeenCalled();
    expect(last().setData).toHaveBeenCalledWith([[NOW.getTime() / 1000, NOW.getTime() / 1000 + 15], [27.4, 31]]);
    expect(latest(view.container)).toBe('31.0 %');
  });
});
