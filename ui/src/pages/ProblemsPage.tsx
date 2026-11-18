import './ProblemsPage.css';

import { useEffect, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { DashboardSummary, Problem, ProblemsAnswer, Severity } from '../api/types';
import { AckDialog } from '../components/AckDialog';
import { Banner } from '../components/Banner';
import { BusyRegion } from '../components/BusyRegion';
import { CloseDialog } from '../components/CloseDialog';
import { Empty } from '../components/Empty';
import { Pager } from '../components/Pager';
import { ProblemMeta, problemName, ProblemTitle, resolveText } from '../components/ProblemRow';
import { SearchField } from '../components/SearchField';
import { StaleNote } from '../components/StaleNote';
import { TimeText } from '../components/TimeText';
import { DASH, duration } from '../lib/format';
import { usePaging } from '../lib/paging';
import { ONCE, usePolled } from '../lib/poll';
import { useQueryState } from '../lib/query';
import { Link } from '../lib/router';
import { useSession } from '../session';
import { useNow, useStatus } from '../shell/context';

const LIMIT = 50;
const SEARCH_MAX = 200;
const GROUP_MAX = 63;
const HOST_MAX = 255;
const DASHBOARD_MAX = 200;
const DASHBOARDS_SHOWN = 3;

type StateFilter = 'open' | 'resolved';
type MinSeverity = Exclude<Severity, 'info'>;

const SEVERITY_OPTIONS: { value: MinSeverity | ''; label: string }[] = [
  { value: '', label: 'All severities' },
  { value: 'warning', label: 'Warning and above' },
  { value: 'high', label: 'High and above' },
  { value: 'critical', label: 'Critical' },
];

function parseState(text: string | null): StateFilter {
  return text === 'resolved' ? 'resolved' : 'open';
}

function parseSeverity(text: string | null): MinSeverity | undefined {
  return text === 'warning' || text === 'high' || text === 'critical' ? text : undefined;
}

interface DashboardLinksProps {
  list: { id: string; name: string }[];
  state: StateFilter;
}

function DashboardLinks({ list, state }: DashboardLinksProps) {
  if (list.length === 0) return <span class="dim">{DASH}</span>;
  const shown = list.slice(0, DASHBOARDS_SHOWN);
  const rest = list.slice(DASHBOARDS_SHOWN);
  return (
    <>
      {shown.map((d, i) => {
        const params = new URLSearchParams({ dashboard: d.id });
        if (state === 'resolved') params.set('state', 'resolved');
        return (
          <span key={d.id}>
            {i > 0 && ', '}
            <Link href={`/problems?${params.toString()}`}>{d.name}</Link>
          </span>
        );
      })}
      {rest.length > 0 && (
        <span class="dim" title={rest.map((d) => d.name).join(', ')}>
          {` and ${rest.length} more`}
        </span>
      )}
    </>
  );
}

interface TableProps {
  rows: Problem[];
  dashboards: ProblemsAnswer['dashboards'];
  state: StateFilter;
  now: Date;
  canAct: boolean;
  /** True while the answer for a new query is outstanding: nothing can be acted on. */
  busy: boolean;
  onAck: (p: Problem) => void;
  onClose: (p: Problem) => void;
}

function ProblemTable({ rows, dashboards, state, now, canAct, busy, onAck, onClose }: TableProps) {
  const open = state === 'open';
  return (
    <div class="table-scroll">
      <table class="problems-table" aria-label="Problems">
        <thead>
          <tr>
            <th scope="col">
              <span class="visually-hidden">Severity</span>
            </th>
            <th scope="col">Host</th>
            <th scope="col">Problem</th>
            <th scope="col">Dashboards</th>
            <th scope="col">Started</th>
            <th scope="col">Duration</th>
            {open && canAct && <th scope="col">Action</th>}
          </tr>
        </thead>
        <tbody>
          {rows.map((p) => {
            const acknowledged = p.acknowledged_at !== null;
            const reason = resolveText(p);
            const end = p.resolved_at === null ? now : new Date(p.resolved_at);
            return (
              <tr key={p.id} class={`sev-${p.severity}${acknowledged ? ' is-acknowledged' : ''}`}>
                <td class="cell-sev" aria-hidden="true" />
                <td>
                  <Link class="problem-host-link" href={`/hosts/${seg(p.host)}`}>
                    {p.host}
                  </Link>
                </td>
                <td>
                  <ProblemTitle problem={p} />
                  <ProblemMeta problem={p} now={now} />
                </td>
                <td class="problem-dashboards">
                  <DashboardLinks
                    list={Object.hasOwn(dashboards, p.host) ? (dashboards[p.host] ?? []) : []}
                    state={state}
                  />
                </td>
                <td>
                  <TimeText iso={p.started_at} now={now} />
                </td>
                <td class="problem-end">
                  <span class="num">{duration(p.started_at, end)}</span>
                  {reason !== undefined && <small>{reason}</small>}
                  {p.resolve_reason === 'closed_by_operator' &&
                    p.close_comment !== null &&
                    p.close_comment !== '' && <small>{p.close_comment}</small>}
                </td>
                {open && canAct && (
                  <td>
                    <div class="problem-actions">
                      <button
                        type="button"
                        class="btn"
                        disabled={busy}
                        aria-label={`${acknowledged ? 'Acknowledge again' : 'Acknowledge'} ${problemName(p)} on ${p.host}`}
                        onClick={() => {
                          onAck(p);
                        }}
                      >
                        {acknowledged ? 'Acknowledge again' : 'Acknowledge'}
                      </button>
                      <button
                        type="button"
                        class="btn"
                        disabled={busy}
                        aria-label={`Close problem ${problemName(p)} on ${p.host}`}
                        onClick={() => {
                          onClose(p);
                        }}
                      >
                        Close…
                      </button>
                    </div>
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** An answer together with the tab it was fetched for, so rows and tab can never disagree. */
type Loaded = ProblemsAnswer & { forState: StateFilter };

type DialogState = { kind: 'ack' | 'close'; problem: Problem } | undefined;

export function ProblemsPage() {
  const [query, setQuery] = useQueryState();
  const now = useNow();
  const { reloadStatus } = useStatus();
  const { can } = useSession();
  const canAct = can('operator');

  const state = parseState(query.get('state'));
  const severity = parseSeverity(query.get('severity'));
  const paging = usePaging('offset', LIMIT);
  const offset = paging.offset;
  const rawDashboard = query.get('dashboard') ?? '';
  const rawGroup = query.get('group') ?? '';
  const rawHost = query.get('host') ?? '';
  const rawSearch = query.get('search') ?? '';
  // Values longer than the server accepts are not sent (it would refuse them).
  const dropped = [
    rawDashboard.length > DASHBOARD_MAX ? 'dashboard' : '',
    rawGroup.length > GROUP_MAX ? 'group' : '',
    rawHost.length > HOST_MAX ? 'host' : '',
    rawSearch.length > SEARCH_MAX ? 'search' : '',
  ].filter((w) => w !== '');
  const invalid = dropped.length > 0;
  const dashboard = rawDashboard.length > DASHBOARD_MAX ? '' : rawDashboard;
  const group = rawGroup.length > GROUP_MAX ? '' : rawGroup;
  const host = rawHost.length > HOST_MAX ? '' : rawHost;
  const search = rawSearch.length > SEARCH_MAX ? '' : rawSearch;

  const clearFiltersHref = state === 'resolved' ? '/problems?state=resolved' : '/problems';

  const problems = usePolled(
    (signal) =>
      api
        .get<ProblemsAnswer>(
          '/problems',
          {
            state,
            severity,
            dashboard: dashboard === '' ? undefined : dashboard,
            group: group === '' ? undefined : group,
            host: host === '' ? undefined : host,
            search: search === '' ? undefined : search,
            limit: LIMIT,
            offset,
          },
          signal,
        )
        .then((a): Loaded => ({ ...a, forState: state })),
    [state, severity, dashboard, group, host, search, offset],
  );
  // The lists of the filters are loaded once per mount; "Try again" and a tab that becomes visible
  // after a failure load them again.
  const dashboardList = usePolled(
    (signal) => api.get<{ items: DashboardSummary[] }>('/dashboards', undefined, signal),
    [],
    ONCE,
  );
  const groupList = usePolled(
    (signal) => api.get<{ items: string[] }>('/host-groups', undefined, signal),
    [],
    ONCE,
  );

  const [dialog, setDialog] = useState<DialogState>(undefined);
  const [notice, setNotice] = useState<string | undefined>(undefined);

  // The notice belongs to the list as it was when the action ran: a new filter ends it.
  useEffect(() => {
    setNotice(undefined);
  }, [state, severity, dashboard, group, host, search, offset]);

  const answer = problems.data;
  // The tab the rows belong to; while a new query is pending it is not the URL's.
  const answerState = answer?.forState ?? state;
  // Fewer problems than the offset: the last page that exists replaces it (once the answer is in).
  const { beyond, shownOffset } = paging.correct(answer?.total, problems.pending);

  const failed = answer === undefined && problems.error !== undefined;
  const rejected = failed && problems.error?.status === 400;

  const dashboardItems = dashboardList.data?.items ?? [];
  const dashboardOptions =
    dashboard !== '' && !dashboardItems.some((d) => d.id === dashboard)
      ? [...dashboardItems, { id: dashboard, name: dashboard }]
      : dashboardItems;
  const groupNames = groupList.data?.items ?? [];
  const groupOptions = group !== '' && !groupNames.includes(group) ? [...groupNames, group] : groupNames;

  const closeDialog = () => {
    setDialog(undefined);
    // A refused action (a problem that was resolved meanwhile) leaves the list out of date.
    problems.reload();
  };
  const done = (text: string) => {
    setDialog(undefined);
    setNotice(text);
    problems.reload();
    // The badge of the navigation counts the unacknowledged problems.
    reloadStatus();
  };

  return (
    <>
      <h1>Problems</h1>
      <div class="problems-tabs" role="group" aria-label="Problems to show">
        {(['open', 'resolved'] as const).map((s) => (
          <button
            key={s}
            type="button"
            class="btn"
            aria-pressed={state === s}
            onClick={() => {
              if (state !== s) setQuery({ state: s === 'open' ? undefined : s });
            }}
          >
            {s === 'open' ? 'Open' : 'Resolved'}
          </button>
        ))}
      </div>
      <div class="problems-tools">
        <SearchField
          urlValue={search}
          onCommit={(text) => setQuery({ search: text === '' ? undefined : text })}
          label="Search"
          placeholder="Host, problem or summary"
          class="problems-search"
          maxLength={SEARCH_MAX}
        />
        <label>
          Severity
          <select
            value={severity ?? ''}
            onChange={(e) => setQuery({ severity: e.currentTarget.value || undefined })}
          >
            {SEVERITY_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <label>
          Dashboard
          <select
            value={dashboard}
            onChange={(e) => setQuery({ dashboard: e.currentTarget.value || undefined })}
          >
            <option value="">All dashboards</option>
            {dashboardOptions.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Group
          <select value={group} onChange={(e) => setQuery({ group: e.currentTarget.value || undefined })}>
            <option value="">All groups</option>
            {groupOptions.map((g) => (
              <option key={g} value={g}>
                {g}
              </option>
            ))}
          </select>
        </label>
      </div>
      {host !== '' && (
        <span class="problems-chip">
          {`Host: ${host}`}
          <button
            type="button"
            class="btn"
            aria-label="Remove host filter"
            onClick={() => {
              setQuery({ host: undefined });
            }}
          >
            ×
          </button>
        </span>
      )}

      {invalid && (
        <Banner kind="warning">
          {dropped.map((what) => (
            <span key={what}>The {what} in the address is not valid. </span>
          ))}
          <Link href={clearFiltersHref}>Clear filters</Link>
        </Banner>
      )}
      {notice !== undefined && !problems.pending && <Banner kind="info">{notice}</Banner>}
      {failed &&
        (rejected ? (
          <Banner kind="warning">
            {problems.error?.message ?? 'The filter is not valid.'}{' '}
            <Link href={clearFiltersHref}>Clear filters</Link>
          </Banner>
        ) : (
          <Banner kind="warning">
            Could not load the problems.{' '}
            <button
              type="button"
              class="btn"
              onClick={() => {
                problems.reload();
                if (dashboardList.data === undefined) dashboardList.reload();
                if (groupList.data === undefined) groupList.reload();
              }}
            >
              Try again
            </button>
          </Banner>
        ))}
      {answer !== undefined && (
        // Everything below comes from the last answer: while the answer for a new query is
        // outstanding it is dimmed and marked busy (the empty sentence is not shown at all).
        <BusyRegion pending={problems.pending} class="problems-result">
          <StaleNote of={problems} />
          {answer.total === 0 ? (
            !problems.pending && (
              <Empty>{answerState === 'open' ? 'No open problems.' : 'No resolved problems match.'}</Empty>
            )
          ) : beyond ? null : (
            <>
              {answer.items.length > 0 ? (
                <ProblemTable
                  rows={answer.items}
                  dashboards={answer.dashboards}
                  state={answerState}
                  now={now}
                  canAct={canAct}
                  busy={problems.pending}
                  onAck={(p) => {
                    setDialog({ kind: 'ack', problem: p });
                  }}
                  onClose={(p) => {
                    setDialog({ kind: 'close', problem: p });
                  }}
                />
              ) : (
                !problems.pending && <Empty>Nothing on this page.</Empty>
              )}
              <p class="problems-total num">
                {answer.total === 1 ? '1 problem' : `${answer.total} problems`}
              </p>
              <Pager
                total={answer.total}
                limit={LIMIT}
                offset={shownOffset}
                disabled={problems.pending}
                onChange={paging.set}
              />
            </>
          )}
        </BusyRegion>
      )}
      {dialog?.kind === 'ack' && (
        <AckDialog
          problem={dialog.problem}
          onDone={() => {
            done('Problem acknowledged.');
          }}
          onClose={closeDialog}
        />
      )}
      {dialog?.kind === 'close' && (
        <CloseDialog
          problem={dialog.problem}
          onDone={() => {
            done('Problem closed.');
          }}
          onClose={closeDialog}
        />
      )}
    </>
  );
}
