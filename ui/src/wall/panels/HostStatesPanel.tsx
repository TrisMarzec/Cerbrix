import type { DashboardHost, HostState, HostStatesPanelData, Severity } from '../../api/types';
import { squaresThatFit } from '../fit';
import { box, px, useFit } from '../measure';

/** What the panel reads from the layout: the body and the hidden samples, in pixels. */
interface Layout {
  width: number;
  height: number;
  /** The height of the sentence. */
  line: number;
  /** The square's computed size and its right margin, which is the gap of the grid. */
  square: number;
  gap: number;
  /** The width of "+N" for the largest N this panel can show. */
  more: number;
}

function measureLayout(body: HTMLDivElement): Layout {
  const { width, height } = box(body);
  const square = body.querySelector('.wall-square.wall-sample');
  const style = square === null ? null : getComputedStyle(square);
  return {
    width,
    height,
    line: box(body.querySelector('.wall-hosts-sentence.wall-sample')).height,
    square: style === null ? 0 : px(style.width),
    gap: style === null ? 0 : px(style.marginRight),
    more: box(body.querySelector('.wall-squares-more.wall-sample')).width,
  };
}

/** The squares shown and the cells the "+N" spans after them; null when only the sentence fits. */
interface Squares {
  shown: number;
  /** How many hosts "+N" stands for; 0 without it. */
  left: number;
  span: number;
}

function arrange(hosts: number, total: number, layout: Layout): Squares | null {
  const { width, height, line, square, gap } = layout;
  const area = height - line - gap;
  const cells = squaresThatFit(width, area, square, gap);
  const extra = Math.max(0, total - hosts);
  if (hosts > 0 && extra === 0 && hosts <= cells) return { shown: hosts, left: 0, span: 0 };
  // One row of the area: as many cells as fit across.
  const columns = squaresThatFit(width, square, square, gap);
  const span = Math.max(1, Math.ceil((layout.more + gap) / (square + gap)));
  const shown = Math.min(hosts, cells - span);
  if (hosts === 0 || shown < 1 || span > columns) return null;
  // The summary takes the cells after the squares, or starts the next row when they would wrap;
  // with all cells used it ends the grid exactly.
  const start = (shown % columns) + span > columns ? Math.ceil(shown / columns) * columns : shown;
  if (start + span > cells) return null;
  return { shown, left: hosts - shown + extra, span };
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
  if (total === 1) return without === 1 ? '1 host without problems' : '1 host with a problem';
  return without === total ? `All ${total} without problems` : `${without} of ${total} without problems`;
}

/** How many hosts have no problems, and one square per host, as many as fit. */
export function HostStatesPanel({ panel }: { panel: HostStatesPanelData }) {
  const { body, fit } = useFit(measureLayout);
  const { hosts, total, without_problems: without } = panel;
  const fits = fit !== null && fit.line > 0 && fit.height >= fit.line;
  const squares = fit === null || !fits ? null : arrange(hosts.length, total, fit);
  const shown = squares === null ? [] : hosts.slice(0, squares.shown);
  const healthy = total > 0 && without === total;
  const words = statesInWords(hosts);
  const label =
    squares !== null && squares.left > 0
      ? `${words}; ${shown.length} shown, and ${squares.left} more hosts`
      : words;

  return (
    <>
      <div class="wall-panel-label">Hosts</div>
      <div class="wall-panel-body wall-hosts" ref={body} role="img" aria-label={label}>
        <div class="wall-hosts-sentence wall-sample" aria-hidden="true">
          Hosts
        </div>
        <div class="wall-square wall-sample" aria-hidden="true" />
        {/* The largest N: every host and those the server left out, so "+N" never needs more. */}
        <div class="wall-squares-more wall-sample" aria-hidden="true">
          {`+${Math.max(total, hosts.length)}`}
        </div>
        {fits && (
          <>
            <div class={`wall-hosts-sentence wall-ellipsis${healthy ? ' is-ok' : ''}`}>
              {sentence(total, without)}
            </div>
            {squares !== null && (
              <div class="wall-squares">
                {shown.map((h) => (
                  <div key={h.name} class={squareClass(h.state)} />
                ))}
                {squares.left > 0 && (
                  <div class="wall-squares-more" style={{ gridColumn: `span ${squares.span}` }}>
                    {`+${squares.left}`}
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}
