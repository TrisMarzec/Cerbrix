import './DashboardsPage.css';

import { api, seg } from '../api/client';
import type { DashboardSummary, Display, HostsByState, Severity } from '../api/types';
import { Banner } from '../components/Banner';
import { Empty } from '../components/Empty';
import { StaleNote } from '../components/StaleNote';
import { severityWord } from '../lib/format';
import { usePolled } from '../lib/poll';
import { Link } from '../lib/router';

const MAX_SQUARES = 40;

type SquareClass = keyof HostsByState;

/** Problem classes first, most severe first; healthy hosts last. */
const SQUARE_ORDER: SquareClass[] = ['critical', 'high', 'warning', 'info', 'acknowledged', 'ok'];

const SEVERITIES: Severity[] = ['critical', 'high', 'warning', 'info'];

function squareClassName(kind: SquareClass): string {
  if (kind === 'ok') return 'square is-ok';
  // The counts do not say how severe an acknowledged host was: it is drawn dimmed in the info colour.
  if (kind === 'acknowledged') return 'square sev-info is-acknowledged';
  return `square sev-${kind}`;
}

function squareTitle(kind: SquareClass): string {
  if (kind === 'ok') return 'No problems';
  if (kind === 'acknowledged') return 'Acknowledged';
  return severityWord(kind);
}

/** The strip of squares built from the counts, capped, problem classes first. */
function HostStrip({ dashboard }: { dashboard: DashboardSummary }) {
  const counts = dashboard.hosts_by_state;
  const squares: SquareClass[] = [];
  let withProblems = 0;
  for (const kind of SQUARE_ORDER) {
    const n = Math.max(0, counts[kind]);
    if (kind !== 'ok') withProblems += n;
    for (let i = 0; i < n && squares.length < MAX_SQUARES; i += 1) squares.push(kind);
  }
  const left = Math.max(0, dashboard.host_count - squares.length);
  const hosts = `${dashboard.host_count} ${dashboard.host_count === 1 ? 'host' : 'hosts'}`;
  const problems =
    withProblems === 0
      ? 'none with problems'
      : withProblems === 1
        ? '1 with a problem'
        : `${withProblems} with problems`;
  return (
    <span class="dashboard-hosts">
      <span class="host-squares">
        <span class="squares" role="img" aria-label={`${hosts}, ${problems}`}>
          {squares.map((kind, i) => (
            <i key={i} class={squareClassName(kind)} title={squareTitle(kind)} />
          ))}
        </span>
        {left > 0 && <span class="more">{`and ${left} more`}</span>}
      </span>
      <span class="num">{dashboard.host_count}</span>
    </span>
  );
}

/** "2 critical, 1 warning": most severe first, zeros left out. */
function ProblemsInWords({ dashboard }: { dashboard: DashboardSummary }) {
  const parts: string[] = [];
  let worst: Severity | undefined;
  let open = 0;
  let unacknowledged = 0;
  for (const s of SEVERITIES) {
    const n = dashboard.open_problems[s];
    open += n;
    unacknowledged += dashboard.unacknowledged[s];
    if (n > 0) {
      worst ??= s;
      parts.push(`${n} ${s}`);
    }
  }
  if (worst === undefined) return <span class="state-text is-ok">No problems</span>;
  const allAcknowledged = open > 0 && unacknowledged === 0;
  return (
    <span class={`state-text sev-${worst}${allAcknowledged ? ' is-acknowledged' : ''}`}>
      {parts.join(', ') + (allAcknowledged ? ' (all acknowledged)' : '')}
    </span>
  );
}

const UNIT_WORDS: Record<string, string> = { s: 's', m: 'min', h: 'h', d: 'd' };

/** "90s" → "90 s", "1m30s" → "1 min 30 s", "1d" → "1 d"; anything else as given. */
function every(text: string): string {
  if (!/^(\d+[smhd])+$/.test(text)) return text;
  return [...text.matchAll(/(\d+)([smhd])/g)]
    .map((m) => `${m[1] ?? ''} ${UNIT_WORDS[m[2] ?? ''] ?? ''}`)
    .join(' ');
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function DashboardTable({ items }: { items: DashboardSummary[] }) {
  return (
    <div class="table-scroll">
      <table aria-label="Dashboards">
        <thead>
          <tr>
            <th scope="col">Dashboard</th>
            <th scope="col">Hosts</th>
            <th scope="col">Open problems</th>
            <th scope="col" class="right">
              Priority
            </th>
            <th scope="col">Wall</th>
          </tr>
        </thead>
        <tbody>
          {items.map((d) => (
            <tr key={d.id}>
              <td>
                <Link
                  class="dashboard-name"
                  href={`/problems?${new URLSearchParams({ dashboard: d.id }).toString()}`}
                >
                  {d.name}
                </Link>
                <small class="dashboard-file">{d.file}</small>
              </td>
              <td>
                <HostStrip dashboard={d} />
              </td>
              <td>
                <ProblemsInWords dashboard={d} />
              </td>
              <td class="right num">{d.priority}</td>
              <td>
                {/* The hidden name makes each link's accessible name unique; the row shows it. Two spaces collapse into one. */}
                <Link href={`/wall/${seg(d.id)}`}>
                  Open <span class="visually-hidden">{d.name}</span> on this screen
                </Link>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function settingSentence(display: Display, nameOf: (id: string) => string): string {
  const show = display.show;
  let text =
    'dashboard' in show
      ? `Shows ${nameOf(show.dashboard)}`
      : `Rotates through ${plural(show.rotate.length, 'dashboard', 'dashboards')} every ${every(show.every)}`;
  if (display.switch_on_alarm === null) return `${text}, never switches`;
  text += `, switches on alarms from ${severityWord(display.switch_on_alarm.min_severity)}`;
  // The list of dashboards only matters for switching.
  if (display.dashboards !== null) {
    text += ` (limited to ${plural(display.dashboards.length, 'dashboard', 'dashboards')})`;
  }
  return text;
}

function NowShowing({ display, nameOf }: { display: Display; nameOf: (id: string) => string }) {
  if (display.target === null) {
    return <span class="dim">Following its setting</span>;
  }
  return (
    <span class="display-now">
      <span class="state-text sev-critical">
        {`Showing ${nameOf(display.target)} because of ${display.causes.length === 0 ? 'an alarm' : plural(display.causes.length, 'alarm', 'alarms')}`}
      </span>
      {display.waiting.length > 0 && (
        <span class="dim">{`Waiting: ${display.waiting.map(nameOf).join(', ')}`}</span>
      )}
    </span>
  );
}

function DisplayTable({ items, nameOf }: { items: Display[]; nameOf: (id: string) => string }) {
  return (
    <div class="table-scroll">
      <table aria-label="Wall displays">
        <thead>
          <tr>
            <th scope="col">Display</th>
            <th scope="col">What it shows now</th>
          </tr>
        </thead>
        <tbody>
          {items.map((d) => (
            <tr key={d.id}>
              <th scope="row">
                <span class="dashboard-name">{d.name}</span>
                <small>{settingSentence(d, nameOf)}</small>
              </th>
              <td>
                <NowShowing display={d} nameOf={nameOf} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function DashboardsPage() {
  const dashboards = usePolled(
    (signal) => api.get<{ items: DashboardSummary[] }>('/dashboards', undefined, signal),
    [],
  );
  const displays = usePolled(
    (signal) => api.get<{ items: Display[] }>('/displays', undefined, signal),
    [],
  );

  const names = new Map((dashboards.data?.items ?? []).map((d) => [d.id, d.name]));
  const nameOf = (id: string) => names.get(id) ?? id;

  const dashboardsFailed = dashboards.data === undefined && dashboards.error !== undefined;
  const displaysFailed = displays.data === undefined && displays.error !== undefined;

  return (
    <>
      <h1>Dashboards</h1>
      {dashboardsFailed && (
        <Banner kind="warning">
          Could not load the dashboards.{' '}
          <button type="button" class="btn" onClick={dashboards.reload}>
            Try again
          </button>
        </Banner>
      )}
      <StaleNote of={dashboards} />
      {dashboards.data !== undefined &&
        (dashboards.data.items.length === 0 ? (
          <Empty>
            No dashboards yet. Add a file under <code>dashboards/</code> in the configuration repository.
          </Empty>
        ) : (
          <DashboardTable items={dashboards.data.items} />
        ))}

      {(displays.data !== undefined || displaysFailed) && (
        <section class="displays" aria-labelledby="wall-displays">
          <h2 id="wall-displays">Wall displays</h2>
          {displaysFailed && (
            <Banner kind="warning">
              Could not load the wall displays.{' '}
              <button type="button" class="btn" onClick={displays.reload}>
                Try again
              </button>
            </Banner>
          )}
          <StaleNote of={displays} />
          {displays.data !== undefined &&
            (displays.data.items.length === 0 ? (
              <Empty>No wall displays configured.</Empty>
            ) : dashboards.data !== undefined ? (
              <DisplayTable items={displays.data.items} nameOf={nameOf} />
            ) : dashboardsFailed ? (
              <Empty>Wall displays are shown once the dashboards could be loaded.</Empty>
            ) : null)}
        </section>
      )}
    </>
  );
}
