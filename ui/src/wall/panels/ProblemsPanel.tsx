import type { Problem, ProblemsPanelData } from '../../api/types';
import { problemName } from '../../components/ProblemRow';
import { duration, severityWord } from '../../lib/format';
import { rowsThatFit } from '../fit';
import { box, useFit } from '../measure';

/** What the panel reads from the layout: the body and the hidden samples, in pixels. */
interface Layout {
  width: number;
  height: number;
  /** The height of a two-line problem row. */
  row: number;
  /** The width a row needs for an 8-character host name and the duration side by side. */
  rowWidth: number;
  /** The height of the one-line summary. */
  line: number;
  /** "No open problems" at the 48 size. */
  empty: { width: number; height: number };
}

function measureLayout(body: HTMLDivElement): Layout {
  const { width, height } = box(body);
  const row = box(body.querySelector('.wall-problem.wall-sample'));
  return {
    width,
    height,
    row: row.height,
    rowWidth: row.width,
    line: box(body.querySelector('.wall-problems-more.wall-sample')).height,
    empty: box(body.querySelector('.wall-problems-empty.wall-sample')),
  };
}

/** The trigger with its instance, and the summary when it adds something. */
function problemText(p: Problem): string {
  const name = problemName(p);
  return p.summary === '' || p.summary === p.trigger || p.summary === name ? name : `${name} — ${p.summary}`;
}

/** "Critical · acknowledged". Never who acknowledged or why: a wall is read by everyone. */
function problemMeta(p: Problem): string {
  return p.acknowledged_at === null ? severityWord(p.severity) : `${severityWord(p.severity)} · acknowledged`;
}

/** The severity classes of a problem: its colour as `--sev`, at half strength when acknowledged. */
function sevClasses(p: Problem): string {
  return `sev-${p.severity}${p.acknowledged_at === null ? '' : ' is-acknowledged'}`;
}

interface RowProps {
  problem: Problem;
  now: Date;
  cause: boolean;
  /** Without the duration, so the host and the severity word keep their room. */
  narrow: boolean;
}

function Row({ problem: p, now, cause, narrow }: RowProps) {
  const text = problemText(p);
  return (
    <div class={`wall-problem ${sevClasses(p)}${cause ? ' is-cause' : ''}`}>
      <div class="wall-problem-main">
        <div class="wall-problem-line">
          <span class="wall-problem-host wall-ellipsis" title={p.host}>
            {p.host}
          </span>
          <span class="wall-problem-text wall-ellipsis" title={text}>
            {text}
          </span>
        </div>
        <div class="wall-problem-meta wall-ellipsis">{problemMeta(p)}</div>
      </div>
      {!narrow && <span class="wall-problem-duration">{duration(p.started_at, now)}</span>}
    </div>
  );
}

/**
 * The samples, measured with the real classes, out of the flow and invisible. The sample row is as
 * wide as its content: an 8-character host and a long duration, the narrowest row that keeps both.
 */
function Samples() {
  return (
    <>
      <div class="wall-problem sev-info wall-sample" aria-hidden="true">
        <div class="wall-problem-main">
          <div class="wall-problem-line">
            <span class="wall-problem-host">web-0000</span>
            <span class="wall-problem-text" />
          </div>
          <div class="wall-problem-meta">Critical</div>
        </div>
        <span class="wall-problem-duration">23 h 59 min</span>
      </div>
      <div class="wall-problems-more wall-sample" aria-hidden="true">
        and 0 more
      </div>
      <div class="wall-problems-empty wall-sample" aria-hidden="true">
        No open problems
      </div>
    </>
  );
}

function openProblems(n: number): string {
  return n === 1 ? '1 open problem' : `${n} open problems`;
}

/** What the panel shows, from the most to the least room. */
type View =
  | { kind: 'rows'; shown: Problem[]; more: number }
  | { kind: 'summary' }
  | { kind: 'label' }
  | { kind: 'empty'; small: boolean };

function view(panel: ProblemsPanelData, layout: Layout | null): View {
  const { problems, more } = panel;
  const total = problems.length + more;
  if (layout === null) return { kind: 'label' };
  const { height, row, line, empty } = layout;
  if (total === 0) {
    if (empty.height <= 0 || height < empty.height) return { kind: 'label' };
    return { kind: 'empty', small: layout.width < empty.width };
  }
  // Every row, then rows with "and N more" below, then the summary alone, then the label.
  if (more === 0 && problems.length <= rowsThatFit(height, row, 0)) {
    return { kind: 'rows', shown: problems, more: 0 };
  }
  if (line <= 0 || height < line) return { kind: 'label' };
  const shown = problems.slice(0, rowsThatFit(height - line, row, 0));
  return shown.length === 0 ? { kind: 'summary' } : { kind: 'rows', shown, more: total - shown.length };
}

interface Props {
  panel: ProblemsPanelData;
  now: Date;
  /** The ids of the alarms that caused a switch to this dashboard. */
  causes: string[];
}

/**
 * The open problems as rows in the server's order (most severe first, acknowledged last), as many
 * as fit, the rest summed up in one last line. With less room the summary stands alone, carrying
 * the severity of the most urgent problem; with less still, the label carries the count.
 */
export function ProblemsPanel({ panel, now, causes }: Props) {
  const { body, fit } = useFit(measureLayout);
  const total = panel.problems.length + panel.more;
  const first = panel.problems[0];
  const shows = view(panel, fit);
  const narrow = fit !== null && fit.width < fit.rowWidth;
  const word = first === undefined ? '' : ` · ${severityWord(first.severity)}`;
  const urgent = first === undefined ? '' : ` ${sevClasses(first)}`;
  const counted = shows.kind === 'label' && total > 0;

  return (
    <>
      <div class={`wall-panel-label${counted ? ` is-count${urgent}` : ''}`}>
        {counted ? `Problems · ${total} open${word}` : 'Problems'}
      </div>
      <div class={`wall-panel-body wall-problems${narrow ? ' is-narrow' : ''}`} ref={body}>
        <Samples />
        {shows.kind === 'empty' && (
          <div class={`wall-problems-empty wall-ellipsis${shows.small ? ' is-small' : ''}`}>No open problems</div>
        )}
        {shows.kind === 'rows' &&
          shows.shown.map((p) => (
            <Row key={p.id} problem={p} now={now} cause={causes.includes(p.id)} narrow={narrow} />
          ))}
        {shows.kind === 'rows' && shows.more > 0 && (
          <div class="wall-problems-more wall-ellipsis">{`and ${shows.more} more`}</div>
        )}
        {shows.kind === 'summary' && (
          <div class={`wall-problems-more wall-ellipsis is-alone${urgent}`}>{`${openProblems(total)}${word}`}</div>
        )}
      </div>
    </>
  );
}
