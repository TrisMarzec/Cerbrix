import 'uplot/dist/uPlot.min.css';
import './Graph.css';

import uPlot from 'uplot';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

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
}: Props) {
  const container = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | undefined>(undefined);
  const shown = useRef<Column | undefined>(undefined);
  const span = useRef(0);
  const [hover, setHover] = useState<Hover | undefined>(undefined);

  const hasData = points !== null && points.length > 0;
  const data = useMemo(
    () => (hasData ? columns(points, stepSecs) : undefined),
    [points, stepSecs, hasData],
  );
  const dataRef = useRef(data);
  dataRef.current = data;
  const thresholdKey = JSON.stringify(thresholds);

  useEffect(() => {
    const target = container.current;
    const initial = dataRef.current;
    if (target === null || initial === undefined) return undefined;

    const styles = getComputedStyle(document.documentElement);
    const dim = colour(styles, '--dim');
    const line = colour(styles, '--line');
    const font = getComputedStyle(target).fontFamily || 'sans-serif';
    const limits = thresholds.map((t) => ({
      trigger: t.trigger,
      value: t.value,
      stroke: colour(styles, SEVERITY_PROPERTY[t.severity]),
    }));
    span.current = (initial[0].at(-1) ?? 0) - (initial[0][0] ?? 0);
    shown.current = initial;

    const axisLook = { stroke: dim, grid: { stroke: line, width: 1 }, ticks: { stroke: line } };
    const options: uPlot.Options = {
      width: target.clientWidth || DEFAULT_WIDTH,
      height,
      legend: { show: false },
      cursor: { drag: { x: false, y: false } },
      scales: {
        x: { time: true },
        y: { range: (_u, min, max) => yRange(thresholds, min, max) },
      },
      axes: [
        {
          ...axisLook,
          // The visible span is the span of the data: the plot cannot be zoomed.
          values: (_u, splits) => tickLabels(splits, span.current),
        },
        { ...axisLook, values: (_u, splits) => splits.map((v) => value(unitKey, v)) },
      ],
      series: [{}, { stroke: colour(styles, '--git'), width: 2, points: { show: false } }],
      hooks: {
        draw: [
          (u) => {
            const ratio = window.devicePixelRatio || 1;
            const { ctx, bbox } = u;
            ctx.save();
            ctx.lineWidth = ratio;
            ctx.setLineDash([6 * ratio, 4 * ratio]);
            const size = Math.round(LABEL_PX * ratio);
            const gap = LABEL_GAP_PX * ratio;
            ctx.font = `${size}px ${font}`;
            // Labels sit inside the plot at its right edge, clear of the y axis and its values.
            ctx.textAlign = 'right';
            const right = bbox.left + bbox.width - LABEL_EDGE_PX * ratio;
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
              const above = y - gap - size >= bbox.top;
              ctx.textBaseline = above ? 'bottom' : 'top';
              ctx.fillText(limit.trigger, right, above ? y - gap : y + gap);
            }
            ctx.restore();
          },
        ],
        setCursor: [
          (u) => {
            const i = u.cursor.idx;
            const at = i === null || i === undefined ? undefined : u.data[0][i];
            const y = i === null || i === undefined ? undefined : u.data[1]?.[i];
            setHover(at === undefined || y === undefined || y === null ? undefined : { at, value: y });
          },
        ],
      },
    };

    const created = new uPlot(options, initial, target);
    plot.current = created;

    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== 'undefined') {
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
    };
  }, [hasData, thresholdKey, unitKey, height]);

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
    <div class="graph">
      <div class="graph-canvas" role="img" aria-label={description} ref={container} />
      <p class="graph-readout num" data-testid="graph-readout" aria-hidden="true">
        {hover !== undefined &&
          `${value(unitKey, hover.value)} · ${clock(new Date(hover.at * 1000).toISOString(), now)}`}
      </p>
      {thresholds.length > 0 && (
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
