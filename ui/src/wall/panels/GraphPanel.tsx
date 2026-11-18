import type { GraphPanelData } from '../../api/types';
import { Graph } from '../../components/Graph';
import { value } from '../../lib/format';
import { box, useFit } from '../measure';
import { useWallUnit } from '../unit';
import { rangeWords, seriesLabel } from './series';

/** The axis font at 1920, in --u: the smallest wall size. */
const AXIS_FONT_U = 24;
/** A plot needs this many axis font sizes of height: the x axis and a little room above it. */
const PLOT_MIN_FONTS = 4;

/** What the panel reads from the layout, in pixels. */
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

/** A string that changes when the panel shows another series: a new one gets a new plot. */
function identity(panel: GraphPanelData): string {
  const params = Object.keys(panel.params)
    .sort()
    .map((k) => [k, panel.params[k]]);
  return JSON.stringify([panel.host, panel.item, params, panel.range]);
}

interface Props {
  panel: GraphPanelData;
  now: Date;
}

/**
 * The history of one item of one host as a line, with the dashed threshold lines of its triggers,
 * filling the panel, and the latest value at the right of the label. A body too low for a plot
 * leaves the label alone, which still carries the latest value.
 */
export function GraphPanel({ panel, now }: Props) {
  const u = useWallUnit();
  const fontPx = AXIS_FONT_U * u;
  const { body, fit } = useFit(measureLayout);
  const { points } = panel;
  const last = points === null ? undefined : points.at(-1);
  const latest = last === undefined ? null : value(panel.item, last[1]);

  let shows: 'graph' | 'no-data' | 'label' = 'label';
  if (fit !== null && last !== undefined && fit.height >= PLOT_MIN_FONTS * fontPx) shows = 'graph';
  if (fit !== null && last === undefined && fit.noData > 0 && fit.height >= fit.noData) shows = 'no-data';

  return (
    <>
      <div class="wall-panel-label has-latest">
        <span class="wall-panel-label-text">{seriesLabel(panel.host, panel.item, panel.params)}</span>
        {latest !== null && <span class="wall-panel-latest">{latest}</span>}
      </div>
      <div class="wall-panel-body wall-graph" ref={body}>
        <div class="wall-nodata wall-sample" aria-hidden="true">
          No data
        </div>
        {shows === 'no-data' && <div class="wall-nodata">No data</div>}
        {shows === 'graph' && (
          <Graph
            key={identity(panel)}
            points={points}
            thresholds={panel.thresholds}
            unitKey={panel.item}
            now={now}
            stepSecs={panel.step_secs}
            label={`${panel.item} of ${panel.host} over the last ${rangeWords(panel.range)}`}
            fill
            fontPx={fontPx}
            interactive={false}
          />
        )}
      </div>
    </>
  );
}
