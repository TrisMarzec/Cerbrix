import 'uplot/dist/uPlot.min.css';
import './Graph.css';

import uPlot from 'uplot';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';

import type { Severity, Threshold } from '../api/types';
import { clock, severityWord, value } from '../lib/format';

const DEFAULT_WIDTH = 600;
/**
 * The default height of the plot in pixels. The frame of a graph on the host page reserves this plus
 * the title and readout line as a literal in HostPage.css (.graph-frame min-height); a change here
 * must change it too, and HostPage.test.tsx fails until it does.
 */
export const GRAPH_HEIGHT = 160;
/** A gap of more than this many steps is not drawn as a line. */
const GAP_STEPS = 3;
/** A span longer than this many seconds (a day and a little) labels the x axis with dates. */
const DAY_SPAN_SECS = 25 * 3600;
/** A span shorter than this many seconds labels the x axis with seconds too. */
const SECONDS_SPAN_SECS = 10 * 60;
/** The size of a threshold label and its distance from the line and the edge, in CSS pixels. */
const LABEL_PX = 12;
const LABEL_GAP_PX = 3;
const LABEL_EDGE_PX = 4;
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

interface Props {
  /** `[timestamp_ms, value]`; `null` or empty shows "No data in this range". */
  points: [number, number][] | null;
  thresholds: Threshold[];
  /** The item key; it decides how values are written. */
  unitKey: string;
  height?: number;
  /** The shell's clock, for the time of the crosshair. */
  now: Date;
  /** Seconds between two points; a gap of more than three steps breaks the line. */
  stepSecs?: number;
  /** What the graph shows, for a screen reader; defaults to the unit key. */
  label?: string;
  /**
   * Take the width and height of the container (followed with a `ResizeObserver`) instead of
   * `height`; nothing is drawn while the container has no size. The thresholds are not listed
   * below the plot: their lines carry the severity word.
   */
  fill?: boolean;
  /** The axis and label font size in pixels; the axes and lines scale with it. */
  fontPx?: number;
  /** False: no cursor and no readout line. */
  interactive?: boolean;
}

/** The font size of the desk graph: uPlot's default axis font and the threshold labels. */
const DESK_FONT_PX = 12;

interface Size {
  width: number;
  height: number;
}

/** The size of an element in whole pixels; null when it has no area. */
function sizeOf(el: HTMLElement): Size | null {
  const r = el.getBoundingClientRect();
  const width = Math.floor(r.width);
  const height = Math.floor(r.height);
  return width > 0 && height > 0 ? { width, height } : null;
}

function sameSize(a: Size | null, b: Size | null): boolean {
  return a === b || (a !== null && b !== null && a.width === b.width && a.height === b.height);
}

/** The axis look for a font size: the font, room for the labels and the space between ticks. */
function axisSizes(
  fontPx: number,
  font: string,
  axis: 'x' | 'y',
): Pick<uPlot.Axis, 'font' | 'size' | 'gap' | 'space'> & { ticks: { size: number } } {
  const k = fontPx / DESK_FONT_PX;
  return {
    font: `${fontPx}px ${font}`,
    // uPlot's defaults at 12 px are a size of 50, a gap of 5, ticks of 10 and a space of 50 (x)
    // or 30 (y); x needs height for one line, y width for a value such as "100.0 %".
    size: axis === 'x' ? Math.round(fontPx * 2.25) : Math.round(fontPx * 4.5),
    gap: Math.round(5 * k),
    space: Math.round((axis === 'x' ? 50 : 30) * k),
    ticks: { size: Math.round(10 * k) },
  };
}

const SEVERITY_PROPERTY: Record<Severity, string> = {
  critical: '--critical',
  high: '--high',
  warning: '--warning',
  info: '--dim',
};

/** A custom property of the document, or `currentColor` where it has no value (a test, a bare page). */
function colour(styles: CSSStyleDeclaration, property: string): string {
  const text = styles.getPropertyValue(property).trim();
  return text === '' ? 'currentColor' : text;
}

type Column = [number[], (number | null)[]];

/** The points as uPlot wants them (seconds and values), with a null where the line must break. */
function columns(points: [number, number][], stepSecs: number | undefined): Column {
  const xs: number[] = [];
  const ys: (number | null)[] = [];
  let previous: number | undefined;
  for (const [ms, v] of points) {
    const t = ms / 1000;
    if (
      previous !== undefined &&
      stepSecs !== undefined &&
      stepSecs > 0 &&
      t - previous > GAP_STEPS * stepSecs
    ) {
      xs.push(previous + stepSecs);
      ys.push(null);
    }
    xs.push(t);
    ys.push(Number.isFinite(v) ? v : null);
    previous = t;
  }
  return [xs, ys];
}

function two(n: number): string {
  return String(n).padStart(2, '0');
}

function tick(seconds: number, spanSecs: number): string {
  const t = new Date(seconds * 1000);
  if (spanSecs > DAY_SPAN_SECS) return `${t.getDate()} ${MONTHS[t.getMonth()] ?? ''}`;
  const time = `${two(t.getHours())}:${two(t.getMinutes())}`;
  return spanSecs < SECONDS_SPAN_SECS ? `${time}:${two(t.getSeconds())}` : time;
}

/**
 * The labels of the x axis for the tick positions `splits` (seconds) of a graph spanning
 * `spanSecs`: day and month beyond a day, hours and minutes, with seconds under ten minutes. A
 * label equal to the one before it is left empty, so no label repeats.
 */
export function tickLabels(splits: number[], spanSecs: number): string[] {
  let previous: string | undefined;
  return splits.map((s) => {
    const label = tick(s, spanSecs);
    if (label === previous) return '';
    previous = label;
    return label;
  });
}

/** A y range that holds the data and every threshold, with a little air. */
function yRange(thresholds: Threshold[], min: number, max: number): [number, number] {
  let lo = min;
  let hi = max;
  for (const t of thresholds) {
    lo = Math.min(lo, t.value);
    hi = Math.max(hi, t.value);
  }
  const pad = hi === lo ? Math.abs(hi) * 0.1 || 1 : (hi - lo) * 0.08;
  return [lo - pad, hi + pad];
}

function latest(data: Column): number | null {
  for (let i = data[1].length - 1; i >= 0; i -= 1) {
    const v = data[1][i];
    if (v !== null && v !== undefined) return v;
  }
  return null;
}

interface Hover {
  at: number;
  value: number;
}

/** A line over time with the limits of the triggers as dashed lines. Drawn with uPlot on a canvas. */
export function Graph({
  points,
  thresholds,
  unitKey,
  height = GRAPH_HEIGHT,
  now,
  stepSecs,
  label,
  fill = false,
  fontPx,
  interactive = true,
}: Props) {
  const container = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | undefined>(undefined);
  const shown = useRef<Column | undefined>(undefined);
  const span = useRef(0);
  const [hover, setHover] = useState<Hover | undefined>(undefined);
  // With `fill`: the container's size, and the size the plot was last given.
  const [filled, setFilled] = useState<Size | null>(null);
  const filledRef = useRef(filled);
  filledRef.current = filled;
  const drawn = useRef<Size | null>(null);

  const hasData = points !== null && points.length > 0;
  const data = useMemo(
    () => (hasData ? columns(points, stepSecs) : undefined),
    [points, stepSecs, hasData],
  );
  const dataRef = useRef(data);
  dataRef.current = data;
  const thresholdKey = JSON.stringify(thresholds);
  const sized = !fill || filled !== null;

  useLayoutEffect(() => {
    const target = container.current;
    if (!fill || target === null) return undefined;
    const update = () => {
      const next = sizeOf(target);
      setFilled((previous) => (sameSize(previous, next) ? previous : next));
    };
    update();
    if (typeof ResizeObserver === 'function') {
      const observer = new ResizeObserver(update);
      observer.observe(target);
      return () => observer.disconnect();
    }
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [fill, hasData]);

  useEffect(() => {
    const target = container.current;
    const initial = dataRef.current;
    const size = fill ? filledRef.current : { width: target?.clientWidth || DEFAULT_WIDTH, height };
    if (target === null || initial === undefined || size === null) return undefined;

    const styles = getComputedStyle(document.documentElement);
    const dim = colour(styles, '--dim');
    const line = colour(styles, '--line');
    const font = getComputedStyle(target).fontFamily || 'sans-serif';
    const limits = thresholds.map((t) => ({
      // Without the list below the plot, the line itself says the severity.
      text: fill ? `${t.trigger} · ${severityWord(t.severity)}` : t.trigger,
      value: t.value,
      stroke: colour(styles, SEVERITY_PROPERTY[t.severity]),
    }));
    span.current = (initial[0].at(-1) ?? 0) - (initial[0][0] ?? 0);
    shown.current = initial;
    // Lengths of the desk graph are for a 12 px font; a larger font scales them.
    const scale = fontPx === undefined ? 1 : fontPx / DESK_FONT_PX;

    const axisLook = { stroke: dim, grid: { stroke: line, width: 1 }, ticks: { stroke: line } };
    const look = (axis: 'x' | 'y'): uPlot.Axis => {
      if (fontPx === undefined) return axisLook;
      const { ticks, ...sizes } = axisSizes(fontPx, font, axis);
      return { ...axisLook, ...sizes, ticks: { ...axisLook.ticks, ...ticks } };
    };
    const hooks: uPlot.Hooks.Arrays = {
      draw: [
        (u) => {
          const ratio = window.devicePixelRatio || 1;
          const { ctx, bbox } = u;
          ctx.save();
          ctx.lineWidth = ratio * Math.max(1, scale / 2);
          ctx.setLineDash([6 * scale * ratio, 4 * scale * ratio]);
          const labelPx = Math.round(LABEL_PX * scale * ratio);
          const gap = LABEL_GAP_PX * scale * ratio;
          ctx.font = `${labelPx}px ${font}`;
          // Labels sit inside the plot at its right edge, clear of the y axis and its values.
          ctx.textAlign = 'right';
          const right = bbox.left + bbox.width - LABEL_EDGE_PX * scale * ratio;
          for (const limit of limits) {
            const y = u.valToPos(limit.value, 'y', true);
            if (y < bbox.top || y > bbox.top + bbox.height) continue;
            ctx.strokeStyle = limit.stroke;
            ctx.fillStyle = limit.stroke;
            ctx.beginPath();
            ctx.moveTo(bbox.left, y);
            ctx.lineTo(bbox.left + bbox.width, y);
            ctx.stroke();
            // Just above the line; below it when there is no room above (a line at the top).
            const above = y - gap - labelPx >= bbox.top;
            ctx.textBaseline = above ? 'bottom' : 'top';
            ctx.fillText(limit.text, right, above ? y - gap : y + gap);
          }
          ctx.restore();
        },
      ],
    };
    if (interactive) {
      hooks.setCursor = [
        (u) => {
          const i = u.cursor.idx;
          const at = i === null || i === undefined ? undefined : u.data[0][i];
          const y = i === null || i === undefined ? undefined : u.data[1]?.[i];
          setHover(at === undefined || y === undefined || y === null ? undefined : { at, value: y });
        },
      ];
    }
    const options: uPlot.Options = {
      ...size,
      legend: { show: false },
      cursor: interactive ? { drag: { x: false, y: false } } : { show: false },
      scales: {
        x: { time: true },
        y: { range: (_u, min, max) => yRange(thresholds, min, max) },
      },
      axes: [
        {
          ...look('x'),
          // The visible span is the span of the data: the plot cannot be zoomed.
          values: (_u, splits) => tickLabels(splits, span.current),
        },
        { ...look('y'), values: (_u, splits) => splits.map((v) => value(unitKey, v)) },
      ],
      series: [
        {},
        {
          stroke: colour(styles, '--git'),
          width: fontPx === undefined ? 2 : Math.max(1.5, fontPx / DESK_FONT_PX),
          points: { show: false },
        },
      ],
      hooks,
    };

    const created = new uPlot(options, initial, target);
    plot.current = created;
    drawn.current = size;

    // With `fill` the layout effect measures the container; the desk follows only its width.
    let observer: ResizeObserver | undefined;
    if (!fill && typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver((entries) => {
        const width = Math.floor(entries[0]?.contentRect.width ?? 0);
        if (width > 0) created.setSize({ width, height });
      });
      observer.observe(target);
    }

    return () => {
      observer?.disconnect();
      created.destroy();
      plot.current = undefined;
      shown.current = undefined;
      drawn.current = null;
    };
  }, [hasData, thresholdKey, unitKey, height, fill, sized, fontPx, interactive]);

  useEffect(() => {
    if (!fill || filled === null || plot.current === undefined || sameSize(drawn.current, filled)) return;
    drawn.current = filled;
    plot.current.setSize(filled);
  }, [fill, filled]);

  useEffect(() => {
    if (data === undefined || plot.current === undefined || shown.current === data) return;
    shown.current = data;
    span.current = (data[0].at(-1) ?? 0) - (data[0][0] ?? 0);
    plot.current.setData(data);
  }, [data]);

  if (data === undefined) return <p class="graph-empty">No data in this range</p>;

  const latestValue = latest(data);
  const description = `${label ?? unitKey}, latest ${value(unitKey, latestValue)}, ${
    thresholds.length === 1 ? '1 threshold' : `${thresholds.length} thresholds`
  }`;

  return (
    <div class={fill ? 'graph is-fill' : 'graph'}>
      <div class="graph-canvas" role="img" aria-label={description} ref={container} />
      {interactive && (
        <p class="graph-readout num" data-testid="graph-readout" aria-hidden="true">
          {hover !== undefined &&
            `${value(unitKey, hover.value)} · ${clock(new Date(hover.at * 1000).toISOString(), now)}`}
        </p>
      )}
      {!fill && thresholds.length > 0 && (
        <ul class="graph-limits">
          {thresholds.map((t, i) => (
            <li key={i} class={`sev-${t.severity}`}>
              <span class="graph-limit-severity">{severityWord(t.severity)}</span>
              {` ${t.trigger}: ${t.op} ${value(unitKey, t.value)}`}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
