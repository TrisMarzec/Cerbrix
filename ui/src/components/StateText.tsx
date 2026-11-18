import type { HostState } from '../api/types';
import { severityWord } from '../lib/format';

export function StateText({ state }: { state: HostState | null }) {
  if (state === null) {
    return <span class="state-text is-ok">No problems</span>;
  }
  const what = state.kind === 'no_data' ? state.summary : state.trigger;
  const more = state.open_problems > 1 ? ` and ${state.open_problems - 1} more` : '';
  const ack = state.acknowledged ? ' (acknowledged)' : '';
  return (
    <span class={`state-text sev-${state.severity}${state.acknowledged ? ' is-acknowledged' : ''}`}>
      {`${severityWord(state.severity)}: ${what}${ack}${more}`}
    </span>
  );
}
