import type { Problem, ProblemsPanelData } from '../../api/types';
import { problemName } from '../../components/ProblemRow';
import { duration, severityWord } from '../../lib/format';
import { rowsThatFit } from '../fit';
import { useFit } from '../measure';

/** How many rows fit into the body, measured from the hidden sample row. */
function measureRows(body: HTMLDivElement): number {
  const sample = body.querySelector('.wall-problem.wall-sample');
  const row = sample === null ? 0 : sample.getBoundingClientRect().height;
  return rowsThatFit(body.clientHeight, row, 0);
}

/** The trigger with its instance, and the summary when it adds something. */
function problemText(p: Problem): string {
  const name = problemName(p);
  return p.summary === '' || p.summary === p.trigger || p.summary === name ? name : `${name} — ${p.summary}`;
}

/** "Critical · acknowledged · mount=/". Never who acknowledged or why: a wall is read by everyone. */
function problemMeta(p: Problem): string {
  const parts = [severityWord(p.severity)];
  if (p.acknowledged_at !== null) parts.push('acknowledged');
  if (p.instance !== '') parts.push(p.instance);
  return parts.join(' · ');
}

function Row({ problem: p, now, cause }: { problem: Problem; now: Date; cause: boolean }) {
  const text = problemText(p);
  const classes = [
    'wall-problem',
    `sev-${p.severity}`,
    p.acknowledged_at === null ? '' : 'is-acknowledged',
    cause ? 'is-cause' : '',
  ];
  return (
    <div class={classes.filter((c) => c !== '').join(' ')}>
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
      <span class="wall-problem-duration">{duration(p.started_at, now)}</span>
    </div>
  );
}

/** Measured with the real classes, out of the flow and invisible. */
function SampleRow() {
  return (
    <div class="wall-problem sev-info wall-sample" aria-hidden="true">
      <div class="wall-problem-main">
        <div class="wall-problem-line">
          <span class="wall-problem-host">Host</span>
          <span class="wall-problem-text">Problem</span>
        </div>
        <div class="wall-problem-meta">Severity</div>
      </div>
      <span class="wall-problem-duration">0 min</span>
    </div>
  );
}

function openProblems(n: number): string {
  return n === 1 ? '1 open problem' : `${n} open problems`;
}

interface Props {
  panel: ProblemsPanelData;
  now: Date;
  /** The ids of the alarms that caused a switch to this dashboard. */
  causes: string[];
}

/**
 * The open problems as rows in the server's order (most severe first, acknowledged last), as many
 * as fit; the rest are summed up in one last line.
 */
export function ProblemsPanel({ panel, now, causes }: Props) {
  const { body, fit } = useFit(measureRows);
  const { problems, more } = panel;
  const total = problems.length + more;
  const room = fit ?? 0;

  let shown: Problem[] = [];
  let summary: string | undefined;
  if (room > 0 && total > 0) {
    if (more === 0 && problems.length <= room) {
      shown = problems;
    } else {
      shown = problems.slice(0, room - 1);
      summary = shown.length === 0 ? openProblems(total) : `and ${total - shown.length} more`;
    }
  }

  return (
    <>
      <div class="wall-panel-label">Problems</div>
      <div class="wall-panel-body wall-problems" ref={body}>
        <SampleRow />
        {room > 0 && total === 0 && <div class="wall-problems-empty">No open problems</div>}
        {shown.map((p) => (
          <Row key={p.id} problem={p} now={now} cause={causes.includes(p.id)} />
        ))}
        {summary !== undefined && <div class="wall-problems-more">{summary}</div>}
      </div>
    </>
  );
}
