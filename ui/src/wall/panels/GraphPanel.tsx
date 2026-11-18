import { useState } from 'preact/hooks';

import type { GraphPanelData, Severity, Threshold } from '../../api/types';
import { Graph } from '../../components/Graph';
import { age, severityWord, value } from '../../lib/format';
import { box, px, useFit, useFitOn } from '../measure';
import { useWallUnit } from '../unit';
import { rangeSecs, rangeWords, seriesLabel } from './series';

/** The axis font at 1920, in --u: the smallest wall size. */
const AXIS_FONT_U = 24;
/** A plot needs this many axis font sizes of height: the x axis and a little room above it. */
const PLOT_MIN_FONTS = 4;
/** The latest value is old after the larger of this and three steps. */
const OLD_MS = 10 * 60_000;
const OLD_STEPS = 3;
/** The latest value is shown only beside at least this many characters of the host name. */
const HOST_CHARS = 8;
const RANK: Record<Severity, number> = { critical: 0, high: 1, warning: 2, info: 3 };

/** What the panel reads from the body, in pixels. */
interface Layout {
  height: number;
  /** The height of the "No data" line. */
  noData: number;
}

function measureLayout(body: HTMLDivElement): Layout {
  return {
    height: box(body).height,
    noData: box(body.querySelector('.wall-nodata.wall-sample')).height,
  };
}

/** What the panel reads from the label line, in pixels: its width and its samples' widths. */
interface Line {
  width: number;
  gap: number;
  /** The first characters of the host name, the least the series text keeps. */
  prefix: number;
  latest: number;
  worst: number;
}

function measureLine(line: HTMLDivElement): Line {
  const width = (selector: string) => box(line.querySelector(selector)).width;
  return {
    width: box(line).width,
    gap: px(getComputedStyle(line).columnGap),
    prefix: width('.wall-panel-label-text.wall-sample'),
    latest: width('.wall-panel-latest.wall-sample'),
    worst: width('.wall-panel-worst.wall-sample'),
  };
}

/** A string that changes when the panel shows another series: a new one gets a new plot. */
function identity(panel: GraphPanelData): string {
  const params = Object.keys(panel.params)
    .sort()
    .map((k) => [k, panel.params[k]]);
  return JSON.stringify([panel.host, panel.item, params, panel.range]);
}

/** The most severe threshold, the first of equals. */
function worstOf(thresholds: Threshold[]): Threshold | undefined {
  let worst: Threshold | undefined;
  for (const t of thresholds) if (worst === undefined || RANK[t.severity] < RANK[worst.severity]) worst = t;
  return worst;
}

function thresholdCount(n: number): string {
  return n === 1 ? '1 threshold' : `${n} thresholds`;
}

interface Props {
  panel: GraphPanelData;
  now: Date;
}

/**
 * The history of one item of one host as a line up to now, with the dashed threshold lines of
 * its triggers, filling the panel, and the latest value at the right of the label; a latest
 * value that is no longer fresh says "old" in the warning colour. When a threshold label had no
 * room on the plot, the label line names the worst threshold in words. A body too low for a plot
 * leaves the label alone.
 */
export function GraphPanel({ panel, now }: Props) {
  const u = useWallUnit();
  const fontPx = Math.round(AXIS_FONT_U * u);
  const { body, fit } = useFit(measureLayout);
  const series = identity(panel);
  // Whether the plot of this series wrote every threshold label; true until it says otherwise.
  const [labels, setLabels] = useState<{ series: string; all: boolean }>({ series, all: true });
  const { points } = panel;
  const last = points === null ? undefined : points.at(-1);

  let shows: 'graph' | 'no-data' | 'label' = 'label';
  if (fit !== null && last !== undefined && fit.height >= PLOT_MIN_FONTS * fontPx) shows = 'graph';
  if (fit !== null && last === undefined && fit.noData > 0 && fit.height >= fit.noData) shows = 'no-data';

  const latestValue = last === undefined ? null : value(panel.item, last[1]);
  const old =
    last !== undefined && now.getTime() - last[0] > Math.max(OLD_MS, OLD_STEPS * panel.step_secs * 1000);
  const latestText = latestValue === null ? null : old ? `${latestValue} · old` : latestValue;
  const worst = worstOf(panel.thresholds);
  const unlabelled = shows === 'graph' && !(labels.series === series && labels.all);
  const worstText =
    worst === undefined || !unlabelled ? null : `${severityWord(worst.severity)} at ${value(panel.item, worst.value)}`;
  const prefix = panel.host.slice(0, HOST_CHARS);

  // The series text is cut first; the worst threshold goes before the latest value is ever cut.
  const { body: lineRef, fit: line } = useFitOn(measureLine, `${prefix}|${latestText ?? ''}|${worstText ?? ''}`);
  const fits = (latest: boolean, worstToo: boolean) =>
    line !== null &&
    line.prefix + (latest ? line.gap + line.latest : 0) + (worstToo ? line.gap + line.worst : 0) <= line.width;
  const showLatest = latestText !== null && fits(true, false);
  const showWorst = worstText !== null && fits(showLatest, true);

  const description = `${panel.item} of ${panel.host} over the last ${rangeWords(panel.range)}, latest ${
    latestValue ?? ''
  }${old && last !== undefined ? `, ${age(new Date(last[0]).toISOString(), now)}` : ''}, ${thresholdCount(
    panel.thresholds.length,
  )}`;

  return (
    <>
      <div class="wall-panel-label has-latest" ref={lineRef}>
        <span class="wall-panel-label-text wall-sample" aria-hidden="true">
          {prefix}
        </span>
        {latestText !== null && (
          <span class={`wall-panel-latest wall-sample${old ? ' is-old' : ''}`} aria-hidden="true">
            {latestText}
          </span>
        )}
        {worstText !== null && worst !== undefined && (
          <span class={`wall-panel-worst sev-${worst.severity} wall-sample`} aria-hidden="true">
            {worstText}
          </span>
        )}
        <span class="wall-panel-label-text">{seriesLabel(panel.host, panel.item, panel.params)}</span>
        {showWorst && worst !== undefined && <span class={`wall-panel-worst sev-${worst.severity}`}>{worstText}</span>}
        {showLatest && <span class={`wall-panel-latest${old ? ' is-old' : ''}`}>{latestText}</span>}
      </div>
      <div class="wall-panel-body wall-graph" ref={body}>
        <div class="wall-nodata wall-sample" aria-hidden="true">
          No data
        </div>
        {shows === 'no-data' && <div class="wall-nodata">No data</div>}
        {shows === 'graph' && (
          <Graph
            key={series}
            points={points}
            thresholds={panel.thresholds}
            unitKey={panel.item}
            now={now}
            stepSecs={panel.step_secs}
            description={description}
            fill
            fontPx={fontPx}
            interactive={false}
            until={now.getTime() / 1000}
            spanSecs={rangeSecs(panel.range)}
            onLabels={(all) => setLabels((s) => (s.series === series && s.all === all ? s : { series, all }))}
          />
        )}
      </div>
    </>
  );
}
