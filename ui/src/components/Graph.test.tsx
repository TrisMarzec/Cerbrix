import { act, render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';

import type { Threshold } from '../api/types';
import { NOW } from '../test/fixtures';
import { Graph, tickLabels } from './Graph';

interface Instance {
  opts: uPlot.Options;
  data: uPlot.AlignedData;
  target: HTMLElement;
  setSize: ReturnType<typeof vi.fn>;
  setData: ReturnType<typeof vi.fn>;
  destroy: ReturnType<typeof vi.fn>;
}

const instances: Instance[] = [];

vi.mock('uplot', () => ({
  default: class {
    constructor(opts: uPlot.Options, data: uPlot.AlignedData, target: HTMLElement) {
      const self = this as unknown as Instance;
      self.opts = opts;
      self.data = data;
      self.target = target;
      self.setSize = vi.fn();
      self.setData = vi.fn();
      self.destroy = vi.fn();
      instances.push(self);
    }
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

const MINUTE = 60_000;
const T0 = 1_794_865_800_000;

function points(values: number[], step = MINUTE): [number, number][] {
  return values.map((v, i) => [T0 + i * step, v]);
}

class FakeResizeObserver {
  static all: FakeResizeObserver[] = [];
  callback: ResizeObserverCallback;
  disconnect = vi.fn();
  observe = vi.fn();
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    FakeResizeObserver.all.push(this);
  }
  unobserve = vi.fn();
  fire(width: number) {
    this.callback(
      [{ contentRect: { width } } as unknown as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    );
  }
}

const THRESHOLDS: Threshold[] = [
  { trigger: 'Disk almost full', severity: 'critical', op: '>', value: 90 },
  { trigger: 'Disk filling up', severity: 'warning', op: '>', value: 80 },
];

function last(): Instance {
  const instance = instances.at(-1);
  if (instance === undefined) throw new Error('uPlot was not created');
  return instance;
}

beforeEach(() => {
  instances.length = 0;
  FakeResizeObserver.all = [];
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  const root = document.documentElement.style;
  root.setProperty('--git', 'git-colour');
  root.setProperty('--critical', 'critical-colour');
  root.setProperty('--high', 'high-colour');
  root.setProperty('--warning', 'warning-colour');
  root.setProperty('--dim', 'dim-colour');
  root.setProperty('--line', 'line-colour');
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute('style');
});

function fakeCanvas(width = 400) {
  const calls: string[] = [];
  const texts: { text: string; x: number; y: number; align: string; baseline: string }[] = [];
  const state = { dash: [] as number[], stroke: '', fill: '' };
  const ctx = {
    textAlign: 'start',
    textBaseline: 'alphabetic',
    save: () => calls.push('save'),
    restore: () => calls.push('restore'),
    beginPath: () => calls.push('beginPath'),
    moveTo: (x: number, y: number) => calls.push(`moveTo ${x} ${y}`),
    lineTo: (x: number, y: number) => calls.push(`lineTo ${x} ${y}`),
    stroke: () => calls.push(`stroke ${state.stroke} ${state.dash.join(',')}`),
    fillText: (text: string, x: number, y: number) => {
      calls.push(`fillText ${text} ${state.fill}`);
      texts.push({ text, x, y, align: ctx.textAlign, baseline: ctx.textBaseline });
    },
    setLineDash: (d: number[]) => {
      state.dash = d;
    },
    set strokeStyle(v: string) {
      state.stroke = v;
    },
    set fillStyle(v: string) {
      state.fill = v;
    },
    font: '',
    lineWidth: 1,
  };
  const u = {
    ctx,
    bbox: { left: 10, top: 5, width, height: 100 },
    valToPos: (v: number) => 105 - v,
    cursor: { idx: null as number | null },
  };
  return { u, calls, texts };
}

describe('Graph', () => {
  it.each([[null], [[]]])('says there is no data for %j and creates no plot', (given) => {
    render(<Graph now={NOW} points={given} thresholds={[]} unitKey="cpu.usage" />);
    expect(screen.getByText('No data in this range')).toBeInTheDocument();
    expect(instances).toHaveLength(0);
  });

  it('hands the points to uPlot as seconds and values, with the Git colour for the line', () => {
    render(<Graph now={NOW} points={points([10, 20, 30])} thresholds={[]} unitKey="cpu.usage" />);
    const { opts, data } = last();
    expect(data).toEqual([[T0 / 1000, T0 / 1000 + 60, T0 / 1000 + 120], [10, 20, 30]]);
    expect(opts.series[1]?.stroke).toBe('git-colour');
    expect(opts.legend?.show).toBe(false);
    expect(opts.scales?.['x']?.time).toBe(true);
  });

  it('falls back to currentColor where a custom property is empty', () => {
    document.documentElement.removeAttribute('style');
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" />);
    expect(last().opts.series[1]?.stroke).toBe('currentColor');
  });

  it('uses the width of the container, 600 where it has none (jsdom)', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" height={140} />);
    expect(last().opts.width).toBe(600);
    expect(last().opts.height).toBe(140);
  });

  it('does not connect across a gap of more than three steps', () => {
    const given: [number, number][] = [
      [T0, 1],
      [T0 + 90_000, 2],
      [T0 + 90_000 * 10, 3],
    ];
    render(<Graph now={NOW} points={given} thresholds={[]} unitKey="cpu.usage" stepSecs={90} />);
    const [xs, ys] = last().data as [number[], (number | null)[]];
    expect(ys).toEqual([1, 2, null, 3]);
    expect(xs).toHaveLength(4);
    expect(xs[2]).toBeGreaterThan(xs[1] ?? 0);
    expect(xs[2]).toBeLessThan(xs[3] ?? 0);
  });

  it('keeps a gap of three steps connected', () => {
    const given: [number, number][] = [
      [T0, 1],
      [T0 + 270_000, 2],
    ];
    render(<Graph now={NOW} points={given} thresholds={[]} unitKey="cpu.usage" stepSecs={90} />);
    expect((last().data as number[][])[1]).toEqual([1, 2]);
  });

  it('formats the y axis and the readout with value() of the unit key', () => {
    render(<Graph now={NOW} points={points([29.44, 30])} thresholds={[]} unitKey="cpu.usage" />);
    const axis = last().opts.axes?.[1];
    const values = axis?.values as (u: unknown, splits: number[]) => string[];
    expect(values({}, [0, 50])).toEqual(['0.0 %', '50.0 %']);
  });

  it('draws one dashed line per threshold in its severity colour, labelled with the trigger name', () => {
    render(<Graph now={NOW} points={points([50, 60])} thresholds={THRESHOLDS} unitKey="disk.used_percent" />);
    const hooks = last().opts.hooks?.draw;
    const draw = Array.isArray(hooks) ? hooks[0] : hooks;
    const { u, calls } = fakeCanvas();
    draw?.(u as unknown as uPlot);
    const strokes = calls.filter((c) => c.startsWith('stroke'));
    expect(strokes).toHaveLength(2);
    expect(strokes[0]).toMatch(/^stroke critical-colour \d+,\d+$/);
    expect(strokes[1]).toMatch(/^stroke warning-colour \d+,\d+$/);
    expect(calls).toContainEqual(expect.stringMatching(/^fillText Disk almost full critical-colour/));
    expect(calls).toContainEqual(expect.stringMatching(/^fillText Disk filling up warning-colour/));
  });

  it('draws info thresholds in the dim colour', () => {
    const info: Threshold[] = [{ trigger: 'Note', severity: 'info', op: '>', value: 5 }];
    render(<Graph now={NOW} points={points([1, 2])} thresholds={info} unitKey="x" />);
    const hooks = last().opts.hooks?.draw;
    const draw = Array.isArray(hooks) ? hooks[0] : hooks;
    const { u, calls } = fakeCanvas();
    draw?.(u as unknown as uPlot);
    expect(calls.find((c) => c.startsWith('stroke'))).toMatch(/dim-colour/);
  });

  it('extends the y range to include a threshold outside the data', () => {
    render(<Graph now={NOW} points={points([10, 20])} thresholds={THRESHOLDS} unitKey="disk.used_percent" />);
    const range = last().opts.scales?.['y']?.range as (
      u: unknown,
      min: number,
      max: number,
    ) => [number, number];
    const [lo, hi] = range({}, 10, 20);
    expect(lo).toBeLessThanOrEqual(10);
    expect(hi).toBeGreaterThanOrEqual(90);
  });

  it('gives a flat series a range with some height', () => {
    render(<Graph now={NOW} points={points([5, 5])} thresholds={[]} unitKey="x" />);
    const range = last().opts.scales?.['y']?.range as (
      u: unknown,
      min: number,
      max: number,
    ) => [number, number];
    const [lo, hi] = range({}, 5, 5);
    expect(hi).toBeGreaterThan(lo);
  });

  it('describes the graph for a screen reader', () => {
    render(
      <Graph now={NOW}
        points={points([10, 29.4])}
        thresholds={THRESHOLDS}
        unitKey="cpu.usage"
        label="cpu.usage over the last 6 hours"
      />,
    );
    expect(
      screen.getByRole('img', {
        name: 'cpu.usage over the last 6 hours, latest 29.4 %, 2 thresholds',
      }),
    ).toBeInTheDocument();
  });

  it('shows the value and time under the cursor as text', () => {
    render(<Graph now={NOW} points={points([10, 29.4])} thresholds={[]} unitKey="cpu.usage" />);
    const hooks = last().opts.hooks?.setCursor;
    const setCursor = Array.isArray(hooks) ? hooks[0] : hooks;
    act(() => {
      setCursor?.({
        cursor: { idx: 1 },
        data: last().data,
      } as unknown as uPlot);
    });
    expect(screen.getByTestId('graph-readout')).toHaveTextContent('29.4 %');
    act(() => {
      setCursor?.({ cursor: { idx: null }, data: last().data } as unknown as uPlot);
    });
    expect(screen.getByTestId('graph-readout')).toHaveTextContent('');
  });

  it('destroys the plot on unmount', () => {
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" />);
    const { destroy } = last();
    act(() => {
      view.unmount();
    });
    expect(destroy).toHaveBeenCalledTimes(1);
  });

  it('feeds new points into the same plot', () => {
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" />);
    view.rerender(<Graph now={NOW} points={points([3, 4])} thresholds={[]} unitKey="cpu.usage" />);
    expect(instances).toHaveLength(1);
    expect(last().setData).toHaveBeenCalledWith([[T0 / 1000, T0 / 1000 + 60], [3, 4]]);
  });

  it('resizes the plot when its container resizes', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" height={150} />);
    const observer = FakeResizeObserver.all[0];
    expect(observer?.observe).toHaveBeenCalled();
    observer?.fire(480);
    expect(last().setSize).toHaveBeenCalledWith({ width: 480, height: 150 });
  });

  it('stops observing on unmount', () => {
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" />);
    const observer = FakeResizeObserver.all[0];
    act(() => {
      view.unmount();
    });
    expect(observer?.disconnect).toHaveBeenCalled();
  });

  it('works where ResizeObserver does not exist', () => {
    vi.stubGlobal('ResizeObserver', undefined);
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" />);
    expect(instances).toHaveLength(1);
  });

  it('formats x ticks as hours and minutes up to a day, as day and month beyond', () => {
    render(<Graph now={NOW} points={points([1, 2], 30 * MINUTE)} thresholds={[]} unitKey="x" />);
    const short = last().opts.axes?.[0]?.values as (u: unknown, splits: number[]) => string[];
    const noon = new Date(2026, 10, 17, 9, 5).getTime() / 1000;
    expect(short({}, [noon])).toEqual(['09:05']);

    render(
      <Graph now={NOW}
        points={points([1, 2], 3 * 24 * 3_600_000)}
        thresholds={[]}
        unitKey="x"
      />,
    );
    const long = last().opts.axes?.[0]?.values as (u: unknown, splits: number[]) => string[];
    expect(long({}, [noon])).toEqual(['17 Nov']);
  });
});

describe('Graph labels', () => {
  function drawWith(thresholds: Threshold[], valToPos: (v: number) => number) {
    render(<Graph now={NOW} points={points([50, 60])} thresholds={thresholds} unitKey="disk.used_percent" />);
    const hooks = last().opts.hooks?.draw;
    const draw = Array.isArray(hooks) ? hooks[0] : hooks;
    const canvas = fakeCanvas(400);
    canvas.u.valToPos = valToPos;
    draw?.(canvas.u as unknown as uPlot);
    return canvas.texts;
  }

  it('writes a threshold label inside the plot, right-aligned at the right edge, just above its line', () => {
    const [label] = drawWith([THRESHOLDS[0] as Threshold], () => 60);
    expect(label).toMatchObject({ text: 'Disk almost full', align: 'right', baseline: 'bottom' });
    // The plot spans 10 to 410: the label ends a little before the right edge, above y = 60.
    expect(label?.x).toBeLessThan(410);
    expect(label?.x).toBeGreaterThan(400);
    expect(label?.y).toBeLessThan(60);
  });

  it('writes the label below its line when the line is at the top of the plot', () => {
    const [label] = drawWith([THRESHOLDS[0] as Threshold], () => 7);
    expect(label).toMatchObject({ align: 'right', baseline: 'top' });
    expect(label?.y).toBeGreaterThan(7);
  });
});

describe('tickLabels', () => {
  const at = (h: number, m: number, s = 0) => new Date(2026, 10, 17, h, m, s).getTime() / 1000;

  it('includes the seconds when the span is under ten minutes', () => {
    expect(tickLabels([at(21, 24, 0), at(21, 24, 30), at(21, 25, 0)], 4 * 60)).toEqual([
      '21:24:00',
      '21:24:30',
      '21:25:00',
    ]);
  });

  it('writes hours and minutes from ten minutes on', () => {
    expect(tickLabels([at(21, 20), at(21, 30)], 10 * 60)).toEqual(['21:20', '21:30']);
  });

  it('never writes the same label twice in a row', () => {
    expect(tickLabels([at(21, 24, 0), at(21, 24, 20), at(21, 24, 40), at(21, 25, 0)], 3600)).toEqual([
      '21:24',
      '',
      '',
      '21:25',
    ]);
  });

  it('writes day and month beyond a day, also without repeating them', () => {
    expect(tickLabels([at(0, 0), at(12, 0), at(23, 0)], 3 * 86_400)).toEqual(['17 Nov', '', '']);
  });
});
