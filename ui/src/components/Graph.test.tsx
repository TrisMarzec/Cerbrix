import { act, render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';

import type { Threshold } from '../api/types';
import { NOW } from '../test/fixtures';
import { Graph, placeLabels, tickLabels } from './Graph';

interface Instance {
  opts: uPlot.Options;
  data: uPlot.AlignedData;
  target: HTMLElement;
  setSize: ReturnType<typeof vi.fn>;
  setData: ReturnType<typeof vi.fn>;
  setScale: ReturnType<typeof vi.fn>;
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
      self.setScale = vi.fn();
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

describe('Graph on the wall', () => {
  let rect = { width: 0, height: 0 };

  beforeEach(() => {
    rect = { width: 0, height: 0 };
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return this.classList.contains('graph-canvas')
        ? new DOMRect(0, 0, rect.width, rect.height)
        : new DOMRect(0, 0, 0, 0);
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function draw(instance: Instance) {
    const hooks = instance.opts.hooks?.draw;
    const hook = Array.isArray(hooks) ? hooks[0] : hooks;
    const canvas = fakeCanvas();
    hook?.(canvas.u as unknown as uPlot);
    return canvas;
  }

  it('with fill takes the width and height of its container and follows a resize', () => {
    rect = { width: 500.6, height: 300.2 };
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" height={140} fill />);
    expect(instances).toHaveLength(1);
    expect(last().opts.width).toBe(500);
    expect(last().opts.height).toBe(300);
    rect = { width: 700, height: 350 };
    act(() => {
      for (const o of FakeResizeObserver.all) o.fire(0);
    });
    expect(instances).toHaveLength(1);
    expect(last().setSize).toHaveBeenCalledWith({ width: 700, height: 350 });
  });

  it('with fill draws nothing until its container has a height', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fill />);
    expect(instances).toHaveLength(0);
    rect = { width: 400, height: 200 };
    act(() => {
      for (const o of FakeResizeObserver.all) o.fire(0);
    });
    expect(instances).toHaveLength(1);
    expect(last().opts).toMatchObject({ width: 400, height: 200 });
  });

  it('with fill follows the window where ResizeObserver does not exist, and stops on unmount', () => {
    vi.stubGlobal('ResizeObserver', undefined);
    rect = { width: 400, height: 200 };
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fill />);
    rect = { width: 420, height: 210 };
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });
    expect(last().setSize).toHaveBeenCalledWith({ width: 420, height: 210 });
    act(() => {
      view.unmount();
    });
    last().setSize.mockClear();
    window.dispatchEvent(new Event('resize'));
    expect(last().setSize).not.toHaveBeenCalled();
  });

  it('with fill stops observing and destroys the plot on unmount', () => {
    rect = { width: 400, height: 200 };
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fill />);
    act(() => {
      view.unmount();
    });
    expect(last().destroy).toHaveBeenCalledTimes(1);
    for (const o of FakeResizeObserver.all) expect(o.disconnect).toHaveBeenCalled();
  });

  it('with fill writes the severity word on the line instead of a list below', () => {
    rect = { width: 400, height: 200 };
    const apart: Threshold[] = [
      { trigger: 'Disk almost full', severity: 'critical', op: '>', value: 90 },
      { trigger: 'Disk filling up', severity: 'warning', op: '>', value: 40 },
    ];
    const { container } = render(
      <Graph now={NOW} points={points([50, 60])} thresholds={apart} unitKey="disk.used_percent" fill />,
    );
    expect(container.querySelector('.graph-limits')).toBeNull();
    expect(draw(last()).texts.map((t) => t.text)).toEqual([
      'Disk almost full · Critical',
      'Disk filling up · Warning',
    ]);
  });

  it('without interaction has no cursor, no hover and no readout line, but keeps its description', () => {
    render(
      <Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="cpu.usage" interactive={false} />,
    );
    expect(last().opts.cursor?.show).toBe(false);
    expect(last().opts.hooks?.setCursor).toBeUndefined();
    expect(screen.queryByTestId('graph-readout')).toBeNull();
    expect(screen.getByRole('img', { name: 'cpu.usage, latest 2.0 %, 0 thresholds' })).toBeInTheDocument();
  });

  it('sets the axis font, the axis sizes, the label font and the line width from fontPx', () => {
    render(<Graph now={NOW} points={points([50, 60])} thresholds={THRESHOLDS} unitKey="x" fontPx={36} />);
    const [x, y] = last().opts.axes ?? [];
    expect(x?.font).toMatch(/^36px /);
    expect(y?.font).toMatch(/^36px /);
    expect(x?.size).toBeGreaterThan(36);
    expect(typeof y?.size).toBe('function');
    expect(last().opts.series[1]?.width).toBe(3);
    const { u } = draw(last());
    expect(u.ctx.font).toMatch(/^36px /);
  });

  it('keeps a line width of at least 1.5 for a small font', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fontPx={12} />);
    expect(last().opts.series[1]?.width).toBe(1.5);
  });

  it('creates a new plot when fontPx changes', () => {
    const view = render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fontPx={24} />);
    const first = last();
    view.rerender(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fontPx={30} />);
    expect(first.destroy).toHaveBeenCalledTimes(1);
    expect(instances).toHaveLength(2);
    expect(last().opts.axes?.[0]?.font).toMatch(/^30px /);
  });

  it('keeps the desk look without the wall props', () => {
    const { container } = render(
      <Graph now={NOW} points={points([50, 60])} thresholds={THRESHOLDS} unitKey="disk.used_percent" />,
    );
    const { opts } = last();
    expect(opts.axes?.[0]?.font).toBeUndefined();
    expect(opts.axes?.[0]?.size).toBeUndefined();
    expect(opts.series[1]?.width).toBe(2);
    expect(opts.cursor?.show).toBeUndefined();
    expect(opts.hooks?.setCursor).toHaveLength(1);
    expect(container.querySelector('.graph-limits')).not.toBeNull();
    const { u, texts } = draw(last());
    expect(u.ctx.font).toMatch(/^12px /);
    expect(texts.map((t) => t.text)).toEqual(['Disk almost full', 'Disk filling up']);
  });
});

describe('Graph window and labels on the wall', () => {
  const END = T0 / 1000 + 120;

  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
      () => new DOMRect(0, 0, 400, 200),
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function xRange(instance: Instance): [number, number] {
    const range = instance.opts.scales?.['x']?.range as (u: unknown, min: number, max: number) => [number, number];
    return range({}, 0, 0);
  }

  it('ends the x scale at until and starts it spanSecs before', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" until={END} spanSecs={3600} />);
    expect(xRange(last())).toEqual([END - 3600, END]);
  });

  it('starts the x scale at the first point without spanSecs', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" until={END + 600} />);
    expect(xRange(last())).toEqual([T0 / 1000, END + 600]);
  });

  it('spans the data without until, as on the desk', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" />);
    expect(last().opts.scales?.['x']?.range).toBeUndefined();
  });

  it('moves the window with setScale when until moves by more than a step, never with a new plot', () => {
    // The same points: a poll that brings new ones resets the window through setData instead.
    const same = points([1, 2]);
    const at = (until: number) => (
      <Graph now={NOW} points={same} thresholds={[]} unitKey="x" stepSecs={60} until={until} spanSecs={3600} />
    );
    const view = render(at(END));
    view.rerender(at(END + 30));
    view.rerender(at(END + 60));
    expect(last().setScale).not.toHaveBeenCalled();
    view.rerender(at(END + 61));
    expect(last().setScale).toHaveBeenCalledWith('x', { min: END + 61 - 3600, max: END + 61 });
    expect(instances).toHaveLength(1);
    expect(last().destroy).not.toHaveBeenCalled();
  });

  it('takes a given description for a screen reader', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" description="All of it" />);
    expect(screen.getByRole('img', { name: 'All of it' })).toBeInTheDocument();
  });

  it('with fill leaves out the label of the less severe of two close lines and says so', () => {
    const onLabels = vi.fn();
    const close: Threshold[] = [
      { trigger: 'Disk filling up', severity: 'warning', op: '>', value: 80 },
      { trigger: 'Disk almost full', severity: 'critical', op: '>', value: 90 },
    ];
    render(
      <Graph now={NOW} points={points([50, 60])} thresholds={close} unitKey="x" fill onLabels={onLabels} />,
    );
    const hooks = last().opts.hooks?.draw;
    const draw = Array.isArray(hooks) ? hooks[0] : hooks;
    const { u, texts, calls } = fakeCanvas();
    draw?.(u as unknown as uPlot);
    expect(texts.map((t) => t.text)).toEqual(['Disk almost full · Critical']);
    expect(calls.filter((c) => c.startsWith('stroke'))).toHaveLength(2);
    expect(onLabels).toHaveBeenLastCalledWith(false);
  });

  it('with fill reports when every line has its label', () => {
    const onLabels = vi.fn();
    render(<Graph now={NOW} points={points([50, 60])} thresholds={THRESHOLDS.slice(0, 1)} unitKey="x" fill onLabels={onLabels} />);
    const hooks = last().opts.hooks?.draw;
    const draw = Array.isArray(hooks) ? hooks[0] : hooks;
    draw?.(fakeCanvas().u as unknown as uPlot);
    expect(onLabels).toHaveBeenLastCalledWith(true);
  });

  it('makes the y axis as wide as its widest value in the axis font', () => {
    render(<Graph now={NOW} points={points([1, 2])} thresholds={[]} unitKey="x" fontPx={24} />);
    const size = last().opts.axes?.[1]?.size as (self: unknown, values: string[] | null) => number;
    const fonts: string[] = [];
    const ctx = {
      font: '',
      save: () => undefined,
      restore: () => undefined,
      measureText(text: string) {
        fonts.push(ctx.font);
        return { width: text.length * 10 };
      },
    };
    const narrow = size({ ctx }, ['1.5', '2']);
    const wide = size({ ctx }, ['1.5', '1,234.5 GiB']);
    expect(wide - narrow).toBe(80);
    expect(wide).toBeGreaterThan(110);
    expect(fonts[0]).toMatch(/^24px /);
    expect(size({ ctx }, null)).toBeGreaterThan(24);
  });
});

describe('placeLabels', () => {
  // A plot from 0 to 200, labels 24 high, 3 from their line.
  const place = (lines: { y: number; rank: number }[]) => placeLabels(lines, 24, 3, 0, 200);

  it('puts distant labels above their lines', () => {
    expect(place([{ y: 50, rank: 0 }, { y: 150, rank: 2 }])).toEqual([
      { y: 47, baseline: 'bottom' },
      { y: 147, baseline: 'bottom' },
    ]);
  });

  it('puts the less severe of two close lines below its line when there is room', () => {
    // Critical at 50 takes 23..47; warning at 60 cannot take 33..57, takes 63..87.
    expect(place([{ y: 60, rank: 2 }, { y: 50, rank: 0 }])).toEqual([
      { y: 63, baseline: 'top' },
      { y: 47, baseline: 'bottom' },
    ]);
  });

  it('leaves out the less severe label when neither side is free', () => {
    // Critical at 50 above (23..47), high at 75 above (48..72); warning at 62: 35..59 and 65..89 taken.
    expect(place([{ y: 50, rank: 0 }, { y: 75, rank: 1 }, { y: 62, rank: 2 }])).toEqual([
      { y: 47, baseline: 'bottom' },
      { y: 72, baseline: 'bottom' },
      null,
    ]);
  });

  it('puts a label below a line at the top edge', () => {
    expect(place([{ y: 10, rank: 0 }])).toEqual([{ y: 13, baseline: 'top' }]);
  });

  it('keeps a label above a line at the bottom edge', () => {
    expect(place([{ y: 198, rank: 0 }])).toEqual([{ y: 195, baseline: 'bottom' }]);
  });

  it('places nothing in a plot lower than a label', () => {
    expect(placeLabels([{ y: 10, rank: 0 }], 24, 3, 0, 20)).toEqual([null]);
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
