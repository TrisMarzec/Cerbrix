import './ProblemDialogs.css';

import type { Problem } from '../api/types';

/** The problem a dialog is about: host, trigger, instance and summary, as text. */
export function ProblemBrief({ problem: p }: { problem: Problem }) {
  return (
    <p class="problem-brief">
      <strong>{p.host}</strong>
      {` · ${p.trigger}`}
      {p.instance !== '' && ` · ${p.instance}`}
      <br />
      <span class="dim">{p.summary}</span>
    </p>
  );
}
