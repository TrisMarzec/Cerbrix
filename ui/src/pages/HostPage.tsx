import './HostPage.css';

import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type {
  HostDetail,
  HostTrigger,
  HostValuesAnswer,
  Problem,
  Severity,
  HostValue,
} from '../api/types';
import { AckDialog } from '../components/AckDialog';
import { describeActionError } from '../components/actionError';
import { Banner } from '../components/Banner';
import { CloseDialog } from '../components/CloseDialog';
import { Dialog } from '../components/Dialog';
import { Empty } from '../components/Empty';
import { ProblemRow } from '../components/ProblemRow';
import { StaleNote } from '../components/StaleNote';
import { age, clock, severityWord, value } from '../lib/format';
import { usePolled } from '../lib/poll';
import { useQueryState } from '../lib/query';
import { Link } from '../lib/router';
import { useSession } from '../session';
import { useNow } from '../shell/context';
import {
  DEFAULT_RANGE,
  graphSpecs,
  RANGES,
  seriesId,
  seriesTitle,
  SeriesGraph,
  type Range,
} from './HostGraphs';

const SEVERITY_ORDER: Severity[] = ['critical', 'high', 'warning', 'info'];

function parseRange(text: string | null): Range {
  return RANGES.find((r) => r === text) ?? DEFAULT_RANGE;
}

function worst(problems: Problem[]): Severity | undefined {
  return SEVERITY_ORDER.find((s) => problems.some((p) => p.severity === s));
}

function originText(origin: string): string {
  return origin.startsWith('template:') ? `template: ${origin.slice('template:'.length)}` : origin;
}

function problemName(p: Problem): string {
  return p.instance === '' ? p.trigger : `${p.trigger} ${p.instance}`;
}

interface ReleaseProps {
  host: string;
  onDone: () => void;
  onClose: () => void;
  onGone: () => void;
}

function ReleaseDialog({ host, onDone, onClose, onGone }: ReleaseProps) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const release = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    api.send('POST', `/hosts/${seg(host)}/release`).then(
      () => {
        onDone();
      },
      (error: unknown) => {
        if (typeof error === 'object' && error !== null && 'status' in error && error.status === 404) {
          onGone();
          return;
        }
        setFailure(describeActionError(error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title="Release agent binding"
      closeOnBackdrop={!busy}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <p>
        Release the agent binding of <strong>{host}</strong>? The agent on this host must enrol
        again with a new key.
      </p>
      {failure !== undefined && (
        <p role="alert" class="dialog-error">
          {failure}
        </p>
      )}
      <div class="dialog-actions">
        <button type="button" class="btn" disabled={busy} onClick={onClose}>
          Cancel
        </button>
        <button type="button" class="btn" disabled={busy} onClick={release}>
          Release binding
        </button>
      </div>
    </Dialog>
  );
}

function Head({ host, problems, now }: { host: HostDetail; problems: Problem[]; now: Date }) {
  const worstSeverity = worst(problems);
  const count = problems.length;
  const allAcknowledged = count > 0 && problems.every((p) => p.acknowledged_at !== null);
  return (
    <div class="panel host-head">
      <div class="host-head-top">
        {worstSeverity === undefined ? (
          <span class="host-pill is-ok">No problems</span>
        ) : (
          <span class={`host-pill sev-${worstSeverity}`}>
            {`${count} open ${count === 1 ? 'problem' : 'problems'} · ${severityWord(worstSeverity)}${allAcknowledged ? ' (acknowledged)' : ''}`}
          </span>
        )}
        {host.managed_by_git && <span class="host-git">Managed by Git</span>}
      </div>
      <dl class="host-facts">
        <div>
          <dt>Address</dt>
          <dd>{host.address ?? '–'}</dd>
        </div>
        <div>
          <dt>Groups</dt>
          <dd>{host.groups.length === 0 ? '–' : host.groups.join(', ')}</dd>
        </div>
        <div>
          <dt>Templates</dt>
          <dd>{host.templates.length === 0 ? '–' : host.templates.join(', ')}</dd>
        </div>
        <div>
          <dt>Defined in</dt>
          <dd>{host.source_file ?? 'Created by hand'}</dd>
        </div>
        <div>
          <dt>Agent version</dt>
          <dd>{host.agent_version ?? 'unknown'}</dd>
        </div>
        <div>
          <dt>Enrolled</dt>
          <dd>{host.enrolled_at === null ? 'not enrolled' : clock(host.enrolled_at, now)}</dd>
        </div>
        <div>
          <dt>Dashboards</dt>
          <dd>
            {host.dashboards.length === 0
              ? '–'
              : host.dashboards.map((d, i) => (
                  <span key={d.id}>
                    {i > 0 && ', '}
                    <Link href={`/problems?dashboard=${encodeURIComponent(d.id)}`}>{d.name}</Link>
                  </span>
                ))}
          </dd>
        </div>
      </dl>
    </div>
  );
}

function TriggerState({ trigger }: { trigger: HostTrigger }) {
  return trigger.firing ? (
    <span class={`sev-${trigger.severity} trigger-firing`}>Firing</span>
  ) : (
    <span class="dim">Quiet</span>
  );
}

type DialogState = { kind: 'ack' | 'close'; problem: Problem } | undefined;

export function HostPage({ name }: { name: string }) {
  const [query, setQuery] = useQueryState();
  const now = useNow();
  const { can } = useSession();
  const range = parseRange(query.get('range'));

  const detail = usePolled(
    (signal) => api.get<HostDetail>(`/hosts/${seg(name)}`, undefined, signal),
    [name],
  );
  const values = usePolled(
    (signal) => api.get<HostValuesAnswer>(`/hosts/${seg(name)}/values`, undefined, signal),
    [name],
  );

  const [dialog, setDialog] = useState<DialogState>(undefined);
  const [releasing, setReleasing] = useState(false);
  const [released, setReleased] = useState(false);
  const [gone, setGone] = useState(false);

  // A problem the server returned for an action, shown until the reload's answer arrives.
  const [patches, setPatches] = useState<Record<string, Problem>>({});

  const host = detail.data;
  useEffect(() => {
    setPatches({});
  }, [host]);

  // The graph list comes only from an answer that has values. A failed poll or an answer without
  // metrics leaves the last good list in place, so the graphs are not torn down and rebuilt.
  const goodValues = useRef<HostValue[] | undefined>(undefined);
  if (values.data?.metrics_available === true) goodValues.current = values.data.items;
  const good = goodValues.current;
  const specs = useMemo(
    () => (host === undefined || good === undefined ? undefined : graphSpecs(host.items, good)),
    [host?.items, good],
  );

  const notFound = gone || detail.error?.status === 404 || values.error?.status === 404;
  if (notFound) {
    return (
      <>
        <h1>{name}</h1>
        <Empty>{`There is no host named ${name}.`}</Empty>
        <p>
          <Link href="/hosts">Back to hosts</Link>
        </p>
      </>
    );
  }

  if (host === undefined) {
    return (
      <>
        <h1>{name}</h1>
        {detail.error !== undefined && (
          <Banner kind="warning">
            Could not load the host.{' '}
            <button type="button" class="btn" onClick={detail.reload}>
              Try again
            </button>
          </Banner>
        )}
      </>
    );
  }

  const closeDialog = () => {
    setDialog(undefined);
    // A refused action (a problem that was resolved meanwhile) leaves the list out of date.
    detail.reload();
  };
  const actionDone = (p: Problem) => {
    setPatches((previous) => ({ ...previous, [p.id]: p }));
    closeDialog();
  };
  const openProblems = host.open_problems
    .map((p) => patches[p.id] ?? p)
    .filter((p) => p.resolved_at === null);
  const canAct = can('operator');
  const valuesAnswered = values.data !== undefined || values.error !== undefined;

  return (
    <>
      <h1>{name}</h1>
      {released && <Banner kind="info">Agent binding released.</Banner>}
      <StaleNote of={detail} />
      <Head host={host} problems={openProblems} now={now} />
      <div class="host-tools">
        <Link href={`/problems?state=resolved&host=${encodeURIComponent(name)}`}>
          Past problems of this host
        </Link>
        {can('admin') && (
          <button
            type="button"
            class="btn"
            onClick={() => {
              setReleased(false);
              setReleasing(true);
            }}
          >
            Release agent binding
          </button>
        )}
      </div>

      <div class="host-columns">
        <div class="host-left">
          <section>
            <h2>Open problems</h2>
            {openProblems.length === 0 ? (
              <Empty>No open problems.</Empty>
            ) : (
              <div class="host-problems">
                {openProblems.map((p) => (
                  <ProblemRow
                    key={p.id}
                    problem={p}
                    now={now}
                    actions={
                      canAct && (
                        <>
                          <button
                            type="button"
                            class="btn"
                            aria-label={`${p.acknowledged_at === null ? 'Acknowledge' : 'Acknowledge again'} ${problemName(p)}`}
                            onClick={() => {
                              setReleased(false);
                              setDialog({ kind: 'ack', problem: p });
                            }}
                          >
                            {p.acknowledged_at === null ? 'Acknowledge' : 'Acknowledge again'}
                          </button>
                          <button
                            type="button"
                            class="btn"
                            aria-label={`Close problem ${problemName(p)}`}
                            onClick={() => {
                              setReleased(false);
                              setDialog({ kind: 'close', problem: p });
                            }}
                          >
                            Close…
                          </button>
                        </>
                      )
                    }
                  />
                ))}
              </div>
            )}
          </section>

          <section>
            <div class="host-history-head">
              <h2>History</h2>
              <div class="host-range" role="group" aria-label="Time range">
                {RANGES.map((r) => (
                  <button
                    key={r}
                    type="button"
                    class="btn"
                    aria-pressed={r === range}
                    onClick={() => {
                      setReleased(false);
                      setQuery({ range: r === DEFAULT_RANGE ? undefined : r });
                    }}
                  >
                    {r.replace('h', ' h').replace('d', ' d')}
                  </button>
                ))}
              </div>
            </div>
            {host.items.length === 0 ? (
              <Empty>This host has no items configured.</Empty>
            ) : (
              (specs !== undefined ? (
                <>
                  {specs.graphs.map((spec) => (
                    <SeriesGraph
                      key={spec.id}
                      host={name}
                      spec={spec}
                      range={range}
                      now={now}
                      valuesHaveKey={(good ?? []).some((v) => v.key === spec.key)}
                    />
                  ))}
                  {specs.waiting.map((key) => (
                    <p key={key} class="dim">{`No values yet for ${key}`}</p>
                  ))}
                </>
              ) : (
                valuesAnswered && (
                  <Empty>Graphs are shown once the current values could be loaded.</Empty>
                )
              ))
            )}
          </section>
        </div>

        <div class="host-right">
          <section>
            <h2>Current values</h2>
            <StaleNote of={values} />
            {values.data?.metrics_available === false ? (
              <Empty>Current values are not available right now.</Empty>
            ) : values.data === undefined ? (
              values.error !== undefined && (
                <Banner kind="warning">
                  Could not load the current values.{' '}
                  <button type="button" class="btn" onClick={values.reload}>
                    Try again
                  </button>
                </Banner>
              )
            ) : values.data.items.length === 0 ? (
              <Empty>No current values.</Empty>
            ) : (
              <table aria-label="Current values">
                <thead>
                  <tr>
                    <th scope="col">Item</th>
                    <th scope="col" class="right">
                      Value
                    </th>
                    <th scope="col" class="right">
                      Age
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {values.data.items.map((v) => {
                    const title = seriesTitle(v.key, v.params);
                    return (
                      <tr key={seriesId(v.key, v.params)}>
                        <td class="host-item">{title}</td>
                        <td class="right num">{value(v.key, v.value)}</td>
                        <td class="right num dim">{age(v.at, now)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </section>

          <section>
            <h2>Triggers</h2>
            {host.triggers.length === 0 ? (
              <Empty>No triggers configured.</Empty>
            ) : (
              <table aria-label="Triggers">
                <thead>
                  <tr>
                    <th scope="col">Name</th>
                    <th scope="col">Expression</th>
                    <th scope="col">Severity</th>
                    <th scope="col">Origin</th>
                    <th scope="col">State</th>
                  </tr>
                </thead>
                <tbody>
                  {host.triggers.map((t, i) => (
                    <tr key={i}>
                      <td class="host-item">{t.name}</td>
                      <td class="host-expression">{t.expression}</td>
                      <td>{severityWord(t.severity)}</td>
                      <td class="host-item">{originText(t.origin)}</td>
                      <td>
                        <TriggerState trigger={t} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        </div>
      </div>

      {dialog?.kind === 'ack' && (
        <AckDialog problem={dialog.problem} onDone={actionDone} onClose={closeDialog} />
      )}
      {dialog?.kind === 'close' && (
        <CloseDialog problem={dialog.problem} onDone={actionDone} onClose={closeDialog} />
      )}
      {releasing && (
        <ReleaseDialog
          host={name}
          onClose={() => {
            setReleasing(false);
          }}
          onGone={() => {
            setReleasing(false);
            setGone(true);
          }}
          onDone={() => {
            setReleasing(false);
            setReleased(true);
            detail.reload();
          }}
        />
      )}
    </>
  );
}
