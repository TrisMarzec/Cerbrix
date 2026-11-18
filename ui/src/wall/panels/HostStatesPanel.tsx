import type { DashboardHost, HostState, HostStatesPanelData, Severity } from '../../api/types';
import { squaresThatFit } from '../fit';
import { px, useFit } from '../measure';

/**
 * How many squares fit below the sentence, from the hidden samples: the sentence's height, the
 * square's computed size and its right margin, which is the gap of the grid. Null when not even the
 * sentence fits.
 */
function measureSquares(body: HTMLDivElement): number | null {
  const line = body.querySelector('.wall-hosts-sentence.wall-sample');
  const square = body.querySelector('.wall-square.wall-sample');
  if (line === null || square === null) return null;
  const lineHeight = line.getBoundingClientRect().height;
  if (body.clientHeight < lineHeight) return null;
  const style = getComputedStyle(square);
  const gap = px(style.marginRight);
  return squaresThatFit(body.clientWidth, body.clientHeight - lineHeight - gap, px(style.width), gap);
}

function squareClass(state: HostState | null): string {
  if (state === null) return 'wall-square is-ok';
  return `wall-square sev-${state.severity}${state.acknowledged ? ' is-acknowledged' : ''}`;
}

const SEVERITIES: Severity[] = ['critical', 'high', 'warning', 'info'];

/** "44 hosts: 2 critical, 1 warning, 1 acknowledged, 40 without problems"; counts of 0 left out. */
function statesInWords(hosts: DashboardHost[]): string {
  if (hosts.length === 0) return 'No hosts';
  const count = (test: (s: HostState | null) => boolean) => hosts.filter((h) => test(h.state)).length;
  const parts = [
    ...SEVERITIES.map((sev): [number, string] => [
      count((s) => s !== null && !s.acknowledged && s.severity === sev),
      sev,
    ]),
    [count((s) => s !== null && s.acknowledged), 'acknowledged'] as [number, string],
    [count((s) => s === null), 'without problems'] as [number, string],
  ]
    .filter(([n]) => n > 0)
    .map(([n, words]) => `${n} ${words}`);
  return `${hosts.length} ${hosts.length === 1 ? 'host' : 'hosts'}: ${parts.join(', ')}`;
}

function sentence(total: number, without: number): string {
  if (total === 0) return 'No hosts';
  return without === total ? `All ${total} without problems` : `${without} of ${total} without problems`;
}

/** How many hosts have no problems, and one square per host, as many as fit. */
export function HostStatesPanel({ panel }: { panel: HostStatesPanelData }) {
  const { body, fit } = useFit(measureSquares);
  const { hosts, total, without_problems: without } = panel;
  const room = fit ?? 0;
  const shown = hosts.length <= room ? hosts : hosts.slice(0, Math.max(0, room - 1));
  const left = hosts.length - shown.length;
  const healthy = total > 0 && without === total;
  const label =
    left > 0 && room > 0
      ? `${statesInWords(hosts)}; ${shown.length} shown, and ${left} more hosts`
      : statesInWords(hosts);

  return (
    <>
      <div class="wall-panel-label">Hosts</div>
      <div class="wall-panel-body wall-hosts" ref={body} role="img" aria-label={label}>
        <div class="wall-hosts-sentence wall-sample" aria-hidden="true">
          Hosts
        </div>
        <div class="wall-square wall-sample" aria-hidden="true" />
        {fit !== null && (
          <>
            <div class={`wall-hosts-sentence wall-ellipsis${healthy ? ' is-ok' : ''}`}>
              {sentence(total, without)}
            </div>
            {room > 0 && hosts.length > 0 && (
              <div class="wall-squares">
                {shown.map((h) => (
                  <div key={h.name} class={squareClass(h.state)} />
                ))}
                {left > 0 && <div class="wall-squares-more">{`+${left}`}</div>}
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}
