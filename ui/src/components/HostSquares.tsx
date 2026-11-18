import type { HostState } from '../api/types';
import { severityWord } from '../lib/format';

export interface SquareHost {
  name: string;
  state: HostState | null;
}

function stateWords(state: HostState | null): string {
  if (state === null) return 'No problems';
  const word = severityWord(state.severity);
  return state.acknowledged ? `${word}, acknowledged` : word;
}

function squareClass(state: HostState | null): string {
  if (state === null) return 'square is-ok';
  return `square sev-${state.severity}${state.acknowledged ? ' is-acknowledged' : ''}`;
}

export function HostSquares({ hosts, max }: { hosts: SquareHost[]; max?: number }) {
  const shown = max === undefined ? hosts : hosts.slice(0, max);
  const left = hosts.length - shown.length;
  const withProblems = hosts.filter((h) => h.state !== null).length;
  const hostsText = `${hosts.length} ${hosts.length === 1 ? 'host' : 'hosts'}`;
  const problemsText =
    withProblems === 0
      ? 'none with problems'
      : withProblems === 1
        ? '1 with a problem'
        : `${withProblems} with problems`;
  const label = `${hostsText}, ${problemsText}`;
  return (
    <span class="host-squares">
      <span class="squares" role="img" aria-label={label}>
        {shown.map((h) => (
          <i key={h.name} class={squareClass(h.state)} title={`${h.name}: ${stateWords(h.state)}`} />
        ))}
      </span>
      {left > 0 && <span class="more">{`and ${left} more`}</span>}
    </span>
  );
}
