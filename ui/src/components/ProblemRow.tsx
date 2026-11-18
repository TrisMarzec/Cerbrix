import type { ComponentChildren } from 'preact';

import type { Problem } from '../api/types';
import { clock, duration, severityWord } from '../lib/format';

/** The name of a problem: its trigger, and the instance when it has one ("Disk almost full mount=/"). */
export function problemName(p: Problem): string {
  return p.instance === '' ? p.trigger : `${p.trigger} ${p.instance}`;
}

/** How a resolved problem ended, in words; undefined for an open one. */
export function resolveText(p: Problem): string | undefined {
  const reason: string | null = p.resolve_reason;
  switch (reason) {
    case 'condition_ended':
      return 'ended by itself';
    case 'host_removed':
      return 'host removed';
    case 'trigger_removed':
      return 'trigger removed';
    case 'closed_by_operator':
      return p.closed_by === null ? 'closed by hand' : `closed by ${p.closed_by}`;
    case null:
      return undefined;
    default:
      // A reason this version does not know is shown as the server sent it.
      return reason;
  }
}

/** The line under the name: "Warning · acknowledged by anna, 14:05 · watching". */
export function ProblemMeta({ problem: p, now }: { problem: Problem; now: Date }) {
  const acknowledged = p.acknowledged_at !== null;
  const by = p.acknowledged_by === null ? '' : ` by ${p.acknowledged_by}`;
  return (
    <div class="problem-meta">
      <span class="problem-severity">{severityWord(p.severity)}</span>
      {acknowledged && <span>{` · acknowledged${by}, ${clock(p.acknowledged_at ?? '', now)}`}</span>}
      {acknowledged && p.ack_comment !== null && p.ack_comment !== '' && (
        <span>{` · ${p.ack_comment}`}</span>
      )}
    </div>
  );
}

/** The name and summary of a problem, in one wrapping line. */
export function ProblemTitle({ problem: p, host }: { problem: Problem; host?: boolean }) {
  return (
    <div class="problem-main">
      {host === true && (
        <span class="problem-host" title={p.host}>
          {p.host}
        </span>
      )}
      <span class="problem-name">{problemName(p)}</span>
      <span class="problem-summary">{p.summary}</span>
    </div>
  );
}

interface Props {
  problem: Problem;
  now: Date;
  showHost?: boolean;
  actions?: ComponentChildren;
}

export function ProblemRow({ problem: p, now, showHost = false, actions }: Props) {
  const acknowledged = p.acknowledged_at !== null;
  const end = p.resolved_at === null ? now : new Date(p.resolved_at);
  return (
    <div class={`problem-row sev-${p.severity}${acknowledged ? ' is-acknowledged' : ''}`}>
      <div class="problem-body">
        <ProblemTitle problem={p} host={showHost} />
        <ProblemMeta problem={p} now={now} />
      </div>
      <span class="problem-duration num">{duration(p.started_at, end)}</span>
      {actions !== undefined && <div class="problem-actions">{actions}</div>}
    </div>
  );
}
