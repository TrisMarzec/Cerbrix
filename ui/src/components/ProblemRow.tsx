import type { ComponentChildren } from 'preact';

import type { Problem } from '../api/types';
import { clock, duration, severityWord } from '../lib/format';

interface Props {
  problem: Problem;
  now: Date;
  showHost?: boolean;
  actions?: ComponentChildren;
}

export function ProblemRow({ problem: p, now, showHost = false, actions }: Props) {
  const acknowledged = p.acknowledged_at !== null;
  const end = p.resolved_at === null ? now : new Date(p.resolved_at);
  const name = p.instance === '' ? p.trigger : `${p.trigger} ${p.instance}`;
  return (
    <div class={`problem-row sev-${p.severity}${acknowledged ? ' is-acknowledged' : ''}`}>
      <div class="problem-body">
        <div class="problem-main">
          {showHost && (
            <span class="problem-host" title={p.host}>
              {p.host}
            </span>
          )}
          <span class="problem-name">{name}</span>
          <span class="problem-summary">{p.summary}</span>
        </div>
        <div class="problem-meta">
          <span class="problem-severity">{severityWord(p.severity)}</span>
          {acknowledged && (
            <span>{` · acknowledged by ${p.acknowledged_by ?? 'unknown'}, ${clock(p.acknowledged_at ?? '', now)}`}</span>
          )}
          {acknowledged && p.ack_comment !== null && p.ack_comment !== '' && (
            <span>{` · ${p.ack_comment}`}</span>
          )}
        </div>
      </div>
      <span class="problem-duration num">{duration(p.started_at, end)}</span>
      {actions !== undefined && <div class="problem-actions">{actions}</div>}
    </div>
  );
}
