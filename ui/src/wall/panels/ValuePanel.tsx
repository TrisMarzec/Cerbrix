import type { ValuePanelData } from '../../api/types';
import { age, value } from '../../lib/format';
import { box, useFitOn } from '../measure';
import { seriesLabel } from './series';

/** A value older than this is marked as old, so a stuck value is not read as current. */
const OLD_MS = 10 * 60_000;
/** The age in the samples: about the widest an age gets, so the age need not be measured again. */
const SAMPLE_AGE = '59 min ago';

/**
 * The size steps from the largest: the value at 120, 90 and 60 with the age under it, at 38 with
 * the age under it, and at 38 with the age beside it, for a one-row panel.
 */
const STEPS = ['l', 'm', 's', 'text', 'inline'] as const;
type Step = (typeof STEPS)[number];

/** What the panel reads from the layout, in pixels. */
interface Layout {
  width: number;
  height: number;
  /** The box of the value block at each step. */
  steps: Record<Step, { width: number; height: number }>;
  /** The height of the "No data" line. */
  noData: number;
}

function measureLayout(body: HTMLDivElement): Layout {
  const { width, height } = box(body);
  const sample = (step: Step) => box(body.querySelector(`.wall-value.wall-sample.is-${step}`));
  return {
    width,
    height,
    steps: { l: sample('l'), m: sample('m'), s: sample('s'), text: sample('text'), inline: sample('inline') },
    noData: box(body.querySelector('.wall-nodata.wall-sample')).height,
  };
}

/**
 * The largest step whose block fits the body. When the value is too wide even at the smallest
 * step that fits the height, that step with `cut`: the value is replaced by "…", never clipped.
 * Undefined when no step fits the height.
 */
function largest(layout: Layout): { step: Step; cut: boolean } | undefined {
  const high = STEPS.filter((step) => {
    const { height } = layout.steps[step];
    return height > 0 && height <= layout.height;
  });
  const wide = high.find((step) => layout.steps[step].width <= layout.width);
  if (wide !== undefined) return { step: wide, cut: false };
  const smallest = high.at(-1);
  return smallest === undefined ? undefined : { step: smallest, cut: true };
}

interface Age {
  text: string;
  old: boolean;
}

interface BlockProps {
  step: Step;
  text: string;
  age: Age | null;
  sample?: boolean;
  /** The value does not fit: "…" in its place, the whole value in the block's accessible name. */
  cut?: string;
}

function Block({ step, text, age: ago, sample = false, cut }: BlockProps) {
  return (
    <div
      class={`wall-value is-${step}${sample ? ' wall-sample' : ''}`}
      aria-hidden={sample ? 'true' : undefined}
      role={cut === undefined ? undefined : 'img'}
      aria-label={cut}
    >
      <span class="wall-value-number">{cut === undefined ? text : '…'}</span>
      {ago !== null && (
        <span class={`wall-value-age${ago.old ? ' is-old' : ''}`}>
          {ago.old ? `${ago.text} · old` : ago.text}
        </span>
      )}
    </div>
  );
}

interface Props {
  panel: ValuePanelData;
  now: Date;
}

/**
 * The latest value of one item, as large as the panel allows, with its age; a value older than
 * ten minutes says so in the warning colour. A body too low for one line leaves the label alone.
 */
export function ValuePanel({ panel, now }: Props) {
  const text = panel.value === null ? null : value(panel.item, panel.value);
  const at = panel.at;
  const ago: Age | null =
    at === null ? null : { text: age(at, now), old: now.getTime() - new Date(at).getTime() > OLD_MS };
  const sampleAge: Age | null = ago === null ? null : { text: SAMPLE_AGE, old: ago.old };
  // The samples show the value and a fixed age: measured again when the value or the mark changes.
  const { body, fit } = useFitOn(measureLayout, `${text ?? ''}|${sampleAge === null ? '' : String(sampleAge.old)}`);
  const step = fit === null || text === null ? undefined : largest(fit);
  const label = seriesLabel(panel.host, panel.item, panel.params);
  const whole = `${label}: ${text ?? ''}${ago === null ? '' : `, ${ago.old ? `${ago.text}, old` : ago.text}`}`;
  const noData = fit !== null && text === null && fit.noData > 0 && fit.height >= fit.noData;

  return (
    <>
      <div class="wall-panel-label">{label}</div>
      <div class="wall-panel-body wall-value-body" ref={body}>
        <div class="wall-nodata wall-sample" aria-hidden="true">
          No data
        </div>
        {text !== null && STEPS.map((s) => <Block key={s} step={s} text={text} age={sampleAge} sample />)}
        {noData && <div class="wall-nodata">No data</div>}
        {step !== undefined && text !== null && (
          <Block step={step.step} text={text} age={ago} cut={step.cut ? whole : undefined} />
        )}
      </div>
    </>
  );
}
