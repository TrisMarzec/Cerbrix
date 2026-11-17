import './ConfigurationPage.css';

import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type {
  ChangeLists,
  Display,
  EnrollmentKey,
  Listed,
  Status,
  SyncChanges,
  SyncHistoryEntry,
  SyncIssue,
  UnknownAgent,
} from '../api/types';
import { Banner } from '../components/Banner';
import { describeActionError } from '../components/actionError';
import { Dialog } from '../components/Dialog';
import { Empty } from '../components/Empty';
import { Pager } from '../components/Pager';
import { SecretDialog } from '../components/SecretDialog';
import { TimeText } from '../components/TimeText';
import { usePolled } from '../lib/poll';
import { useQueryState } from '../lib/query';
import { useSession } from '../session';
import { useNow, useStatus } from '../shell/context';

const HISTORY_LIMIT = 20;
const DESCRIPTION_MAX = 200;
const COPIED_MS = 2000;
const DASH = '–';

const VALIDITIES: { value: string; label: string }[] = [
  { value: '1h', label: '1 hour' },
  { value: '24h', label: '24 hours' },
  { value: '7d', label: '7 days' },
  { value: '30d', label: '30 days' },
];
const DEFAULT_VALIDITY = '24h';

/** A secret shown once; the page holds it only while its dialog is open. */
interface Secret {
  title: string;
  value: string;
  note: string;
}

type OnSecret = (secret: Secret) => void;

function parseOffset(text: string | null): number {
  if (text === null || !/^\d+$/.test(text)) return 0;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : 0;
}

function offsetParam(offset: number): string | undefined {
  return offset === 0 ? undefined : String(offset);
}

function LoadFailed({ what, onRetry }: { what: string; onRetry: () => void }) {
  return (
    <Banner kind="warning">
      {`Could not load ${what}.`}{' '}
      <button type="button" class="btn" onClick={onRetry}>
        Try again
      </button>
    </Banner>
  );
}

function Failure({ text }: { text: string | undefined }) {
  if (text === undefined) return null;
  return (
    <p role="alert" class="config-error">
      {text}
    </p>
  );
}

interface ConfirmProps<T> {
  title: string;
  confirmLabel: string;
  children: ComponentChildren;
  run: () => Promise<T>;
  /** A sentence for a failure this dialog knows; `undefined` leaves it to the general rule. */
  explain?: (e: unknown) => string | undefined;
  onDone: (result: T) => void;
  onClose: () => void;
}

/** Asks first, runs the request once, and stays open with the failure written out. */
function ConfirmDialog<T>({
  title,
  confirmLabel,
  children,
  run,
  explain,
  onDone,
  onClose,
}: ConfirmProps<T>) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const confirm = () => {
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    run().then(
      (result) => {
        onDone(result);
      },
      (e: unknown) => {
        setFailure(explain?.(e) ?? describeActionError(e));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={title}
      closeOnBackdrop={!busy}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <p>{children}</p>
      <Failure text={failure} />
      <div class="dialog-actions">
        <button type="button" class="btn" disabled={busy} onClick={onClose}>
          Cancel
        </button>
        <button type="button" class="btn" disabled={busy} onClick={confirm}>
          {confirmLabel}
        </button>
      </div>
    </Dialog>
  );
}

// ---------------------------------------------------------------- Synchronisation

function copyHash(hash: string, node: HTMLElement | null): Promise<boolean> {
  const selectIt = () => {
    if (node === null) return;
    const selection = window.getSelection();
    selection?.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(node);
    selection?.addRange(range);
  };
  if (navigator.clipboard === undefined) {
    selectIt();
    return Promise.resolve(false);
  }
  return navigator.clipboard.writeText(hash).then(
    () => true,
    () => {
      selectIt();
      return false;
    },
  );
}

function CommitCopy({ hash }: { hash: string }) {
  const code = useRef<HTMLElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [copied, setCopied] = useState(false);

  useEffect(
    () => () => {
      clearTimeout(timer.current);
    },
    [],
  );

  const onCopy = () => {
    void copyHash(hash, code.current).then((ok) => {
      if (!ok) return;
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        setCopied(false);
      }, COPIED_MS);
    });
  };

  return (
    <span class="config-commit">
      <code ref={code}>{hash}</code>
      <button type="button" class="btn" onClick={onCopy}>
        {copied ? 'Copied' : 'Copy'}
      </button>
    </span>
  );
}

function position(issue: SyncIssue): string {
  if (issue.line === null) return DASH;
  return issue.column === null ? String(issue.line) : `${issue.line}:${issue.column}`;
}

function IssueTable({ issues }: { issues: SyncIssue[] }) {
  return (
    <div class="config-issues">
      <table class="config-table" aria-label="Synchronisation issues">
        <thead>
          <tr>
            <th scope="col">File</th>
            <th scope="col">Position</th>
            <th scope="col">Reason</th>
          </tr>
        </thead>
        <tbody>
          {issues.map((issue, i) => (
            <tr key={i}>
              <td>{issue.file ?? DASH}</td>
              <td class="num">{position(issue)}</td>
              <td>{issue.reason}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SyncSection({ status, now }: { status: Status | undefined; now: Date }) {
  let body: ComponentChildren = null;
  if (status !== undefined) {
    if (!status.gitops) {
      body = (
        <p>
          GitOps is off. Hosts are managed with <code>cerbrix-server admin</code>.
        </p>
      );
    } else {
      const sync = status.sync;
      const state =
        sync.status === 'in_sync' ? (
          <span class="config-state-in-sync">In sync</span>
        ) : sync.status === 'out_of_sync' ? (
          <span class="config-state-out-of-sync">Out of sync</span>
        ) : (
          <span class="dim">Not synchronised yet</span>
        );
      body = (
        <>
          <dl class="config-facts">
            <dt>State</dt>
            <dd>{state}</dd>
            <dt>Active commit</dt>
            <dd>
              {sync.active_commit === null ? (
                <span class="dim">none</span>
              ) : (
                <CommitCopy hash={sync.active_commit} />
              )}
            </dd>
            <dt>Last successful synchronisation</dt>
            <dd>
              {sync.last_success_at === null ? 'never' : <TimeText iso={sync.last_success_at} now={now} />}
            </dd>
            <dt>Last attempt</dt>
            <dd>
              {sync.last_attempt_at === null ? 'never' : <TimeText iso={sync.last_attempt_at} now={now} />}
            </dd>
          </dl>
          {sync.issues.length > 0 && <IssueTable issues={sync.issues} />}
        </>
      );
    }
  }
  return (
    <section class="config-section">
      <h2>Synchronisation</h2>
      {body}
    </section>
  );
}

// ---------------------------------------------------------------- Sync history

const KINDS: { key: keyof SyncChanges; one: string; many: string; heading: string }[] = [
  { key: 'hosts', one: 'host', many: 'hosts', heading: 'Hosts' },
  { key: 'templates', one: 'template', many: 'templates', heading: 'Templates' },
  { key: 'notifications', one: 'notification', many: 'notifications', heading: 'Notifications' },
  { key: 'dashboards', one: 'dashboard', many: 'dashboards', heading: 'Dashboards' },
  { key: 'displays', one: 'display', many: 'displays', heading: 'Displays' },
];
const VERBS: (keyof ChangeLists)[] = ['added', 'changed', 'removed'];

function describeChanges(changes: SyncChanges): { sentence: string; groups: string[] } {
  const parts: string[] = [];
  const groups: string[] = [];
  for (const kind of KINDS) {
    for (const verb of VERBS) {
      const names = changes[kind.key][verb];
      if (names.length === 0) continue;
      parts.push(`${names.length} ${names.length === 1 ? kind.one : kind.many} ${verb}`);
      groups.push(`${kind.heading} ${verb}: ${names.join(', ')}`);
    }
  }
  return { sentence: parts.length === 0 ? 'No changes' : parts.join(', '), groups };
}

function HistoryRow({ entry, now }: { entry: SyncHistoryEntry; now: Date }) {
  const { sentence, groups } = describeChanges(entry.changes);
  return (
    <tr>
      <td>
        <code class="config-code" title={entry.commit}>
          {entry.commit.slice(0, 7)}
        </code>
      </td>
      <td>
        <TimeText iso={entry.applied_at} now={now} />
      </td>
      <td>
        <div>{sentence}</div>
        {groups.length > 0 && (
          <details class="config-names">
            <summary>Names</summary>
            <ul>
              {groups.map((g) => (
                <li key={g}>{g}</li>
              ))}
            </ul>
          </details>
        )}
      </td>
    </tr>
  );
}

function HistorySection({ now }: { now: Date }) {
  const [query, setQuery] = useQueryState();
  const offset = parseOffset(query.get('history_offset'));
  const history = usePolled(
    (signal) =>
      api.get<Listed<SyncHistoryEntry>>('/sync/history', { limit: HISTORY_LIMIT, offset }, signal),
    [offset],
  );
  const answer = history.data;
  const failed = history.error !== undefined && answer === undefined;
  const beyond = answer !== undefined && answer.total > 0 && offset >= answer.total;

  return (
    <section class="config-section">
      <h2>Sync history</h2>
      {failed && <LoadFailed what="the sync history" onRetry={history.reload} />}
      {answer !== undefined && !failed && (
        <div
          class={history.pending ? 'config-result is-busy' : 'config-result'}
          aria-busy={history.pending}
        >
          {answer.total === 0 ? (
            !history.pending && <Empty>No revision has been applied yet.</Empty>
          ) : beyond ? null : (
            <>
              {answer.items.length > 0 ? (
                <table class="config-table" aria-label="Sync history">
                  <thead>
                    <tr>
                      <th scope="col">Commit</th>
                      <th scope="col">Applied</th>
                      <th scope="col">Changes</th>
                    </tr>
                  </thead>
                  <tbody>
                    {answer.items.map((entry) => (
                      <HistoryRow key={entry.commit + entry.applied_at} entry={entry} now={now} />
                    ))}
                  </tbody>
                </table>
              ) : (
                !history.pending && <Empty>Nothing on this page.</Empty>
              )}
            </>
          )}
          <Pager
            total={answer.total}
            limit={HISTORY_LIMIT}
            offset={offset}
            onChange={(next) => {
              setQuery({ history_offset: offsetParam(next) });
            }}
          />
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- Unknown agents

function UnknownAgentsSection({ now }: { now: Date }) {
  const agents = usePolled(
    (signal) => api.get<{ items: UnknownAgent[] }>('/unknown-agents', undefined, signal),
    [],
  );
  const answer = agents.data;
  return (
    <section class="config-section">
      <h2>Unknown agents</h2>
      {agents.error !== undefined && answer === undefined && (
        <LoadFailed what="the unknown agents" onRetry={agents.reload} />
      )}
      {answer !== undefined && (
        <>
          <p class="config-note">Agents that tried to enrol under a name that is not a registered host.</p>
          {answer.items.length === 0 ? (
            <Empty>No unknown agents.</Empty>
          ) : (
            <table class="config-table" aria-label="Unknown agents">
              <thead>
                <tr>
                  <th scope="col">Host name</th>
                  <th scope="col">First seen</th>
                  <th scope="col">Last seen</th>
                  <th scope="col" class="right">
                    Attempts
                  </th>
                </tr>
              </thead>
              <tbody>
                {answer.items.map((a) => (
                  <tr key={a.host_name}>
                    <td>{a.host_name}</td>
                    <td>
                      <TimeText iso={a.first_seen} now={now} />
                    </td>
                    <td>
                      <TimeText iso={a.last_seen} now={now} />
                    </td>
                    <td class="right">{a.attempts}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- Enrolment keys

/** The sentence for a description the server would refuse; `undefined` when it is fine. */
function descriptionProblem(text: string): string | undefined {
  if (text.trim() === '') return 'Enter a description.';
  if ([...text].length > DESCRIPTION_MAX) {
    return `The description may have at most ${DESCRIPTION_MAX} characters.`;
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(text)) {
    return 'The description must not contain control characters.';
  }
  return undefined;
}

interface CreateKeyProps {
  onCreated: (key: string) => void;
  onClose: () => void;
}

function CreateKeyDialog({ onCreated, onClose }: CreateKeyProps) {
  const [description, setDescription] = useState('');
  const [validity, setValidity] = useState(DEFAULT_VALIDITY);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const submit = (e: Event) => {
    e.preventDefault();
    if (busy) return;
    const problem = descriptionProblem(description);
    if (problem !== undefined) {
      setFailure(problem);
      return;
    }
    setBusy(true);
    setFailure(undefined);
    api
      .send<EnrollmentKey & { key: string }>('POST', '/enrollment-keys', {
        description,
        expires_in: validity,
      })
      .then(
        (created) => {
          onCreated(created.key);
        },
        (error: unknown) => {
          setFailure(describeActionError(error));
          setBusy(false);
        },
      );
  };

  return (
    <Dialog
      title="Create enrolment key"
      closeOnBackdrop={!busy}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <form class="config-form" onSubmit={submit} noValidate>
        <label>
          Description
          <input
            type="text"
            value={description}
            disabled={busy}
            onInput={(e) => {
              setDescription(e.currentTarget.value);
            }}
          />
        </label>
        <label>
          Validity
          <select
            value={validity}
            disabled={busy}
            onChange={(e) => {
              setValidity(e.currentTarget.value);
            }}
          >
            {VALIDITIES.map((v) => (
              <option key={v.value} value={v.value}>
                {v.label}
              </option>
            ))}
          </select>
        </label>
        <Failure text={failure} />
        <div class="dialog-actions">
          <button type="button" class="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy}>
            Create key
          </button>
        </div>
      </form>
    </Dialog>
  );
}

const KEY_STATE_WORD: Record<EnrollmentKey['state'], string> = {
  active: 'Active',
  expired: 'Expired',
  revoked: 'Revoked',
};

type KeyDialog = { kind: 'create' } | { kind: 'revoke'; key: EnrollmentKey } | undefined;

function KeysSection({ now, onSecret }: { now: Date; onSecret: OnSecret }) {
  const keys = usePolled(
    (signal) => api.get<{ items: EnrollmentKey[] }>('/enrollment-keys', undefined, signal),
    [],
  );
  const [dialog, setDialog] = useState<KeyDialog>(undefined);
  const answer = keys.data;

  return (
    <section class="config-section">
      <h2>Enrolment keys</h2>
      {keys.error !== undefined && answer === undefined && (
        <LoadFailed what="the enrolment keys" onRetry={keys.reload} />
      )}
      <div class="config-toolbar">
        <button
          type="button"
          class="btn"
          onClick={() => {
            setDialog({ kind: 'create' });
          }}
        >
          Create key
        </button>
      </div>
      {answer !== undefined &&
        (answer.items.length === 0 ? (
          <Empty>No enrolment keys.</Empty>
        ) : (
          <table class="config-table" aria-label="Enrolment keys">
            <thead>
              <tr>
                <th scope="col">Description</th>
                <th scope="col">Created</th>
                <th scope="col">Expires</th>
                <th scope="col">State</th>
                <th scope="col">
                  <span class="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {answer.items.map((k) => (
                <tr key={k.id} class={k.state === 'active' ? undefined : 'config-dim-row'}>
                  <td>{k.description}</td>
                  <td>
                    <TimeText iso={k.created_at} now={now} />
                  </td>
                  <td>
                    <TimeText iso={k.expires_at} now={now} />
                  </td>
                  <td>{KEY_STATE_WORD[k.state]}</td>
                  <td class="config-actions">
                    {k.state === 'active' && (
                      <button
                        type="button"
                        class="btn"
                        aria-label={`Revoke key ${k.description}`}
                        onClick={() => {
                          setDialog({ kind: 'revoke', key: k });
                        }}
                      >
                        Revoke
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ))}
      {dialog?.kind === 'create' && (
        <CreateKeyDialog
          onCreated={(key) => {
            setDialog(undefined);
            keys.reload();
            onSecret({
              title: 'Enrolment key',
              value: key,
              note: "Give this key to the agent's installer.",
            });
          }}
          onClose={() => {
            setDialog(undefined);
          }}
        />
      )}
      {dialog?.kind === 'revoke' && (
        <ConfirmDialog
          title="Revoke enrolment key"
          confirmLabel="Revoke key"
          run={() => api.send<void>('DELETE', `/enrollment-keys/${seg(dialog.key.id)}`)}
          onDone={() => {
            setDialog(undefined);
            keys.reload();
          }}
          onClose={() => {
            setDialog(undefined);
          }}
        >
          {`Revoke the key "${dialog.key.description}"? Agents that have not enrolled yet can no longer use it.`}
        </ConfirmDialog>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- Display tokens

const SYNCING = 'The configuration is being synchronised. Try again.';

function isNotFound(e: unknown): boolean {
  return typeof e === 'object' && e !== null && 'status' in e && e.status === 404;
}

function explainSyncing(e: unknown): string | undefined {
  return isNotFound(e) ? SYNCING : undefined;
}

type TokenDialog =
  | { kind: 'replace'; display: Display }
  | { kind: 'revoke'; display: Display }
  | undefined;

function DisplaysSection({ now, onSecret }: { now: Date; onSecret: OnSecret }) {
  const displays = usePolled(
    (signal) => api.get<{ items: Display[] }>('/displays', undefined, signal),
    [],
  );
  const [dialog, setDialog] = useState<TokenDialog>(undefined);
  const [busyId, setBusyId] = useState<string | undefined>(undefined);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const answer = displays.data;

  /** Creates the token and hands it over as the secret to show; the caller passes it to the page. */
  const createToken = async (display: Display): Promise<Secret> => {
    const created = await api.send<{ token: string }>('POST', `/displays/${seg(display.id)}/token`);
    return {
      title: `Token of ${display.name}`,
      value: created.token,
      note: `Open /display/${display.id} on the wall monitor and enter this token.`,
    };
  };

  const createDirectly = (display: Display) => {
    if (busyId !== undefined) return;
    setBusyId(display.id);
    setFailure(undefined);
    createToken(display).then(
      (secret) => {
        setBusyId(undefined);
        displays.reload();
        onSecret(secret);
      },
      (e: unknown) => {
        setFailure(explainSyncing(e) ?? describeActionError(e));
        setBusyId(undefined);
      },
    );
  };

  return (
    <section class="config-section">
      <h2>Display tokens</h2>
      {displays.error !== undefined && answer === undefined && (
        <LoadFailed what="the displays" onRetry={displays.reload} />
      )}
      <Failure text={failure} />
      {answer !== undefined &&
        (answer.items.length === 0 ? (
          <Empty>No wall displays configured.</Empty>
        ) : (
          <table class="config-table" aria-label="Display tokens">
            <thead>
              <tr>
                <th scope="col">Display</th>
                <th scope="col">Token</th>
                <th scope="col">
                  <span class="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {answer.items.map((d) => {
                const has = d.token_created_at !== null;
                return (
                  <tr key={d.id}>
                    <td>
                      <div>{d.name}</div>
                      <small>{d.id}</small>
                    </td>
                    <td>
                      {d.token_created_at === null ? (
                        <span class="dim">No token</span>
                      ) : (
                        <>
                          Created <TimeText iso={d.token_created_at} now={now} />
                        </>
                      )}
                    </td>
                    <td class="config-actions">
                      <button
                        type="button"
                        class="btn"
                        disabled={busyId === d.id}
                        aria-label={`${has ? 'Replace' : 'Create'} token of ${d.name}`}
                        onClick={() => {
                          if (has) {
                            setFailure(undefined);
                            setDialog({ kind: 'replace', display: d });
                          } else {
                            createDirectly(d);
                          }
                        }}
                      >
                        {has ? 'Replace token' : 'Create token'}
                      </button>
                      {has && (
                        <button
                          type="button"
                          class="btn"
                          aria-label={`Revoke token of ${d.name}`}
                          onClick={() => {
                            setFailure(undefined);
                            setDialog({ kind: 'revoke', display: d });
                          }}
                        >
                          Revoke
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ))}
      {dialog?.kind === 'replace' && (
        <ConfirmDialog
          title="Replace display token"
          confirmLabel="Replace token"
          run={() => createToken(dialog.display)}
          explain={explainSyncing}
          onDone={(secret) => {
            setDialog(undefined);
            displays.reload();
            onSecret(secret);
          }}
          onClose={() => {
            setDialog(undefined);
          }}
        >
          {`Replace the token of "${dialog.display.name}"? The current token stops working at once.`}
        </ConfirmDialog>
      )}
      {dialog?.kind === 'revoke' && (
        <ConfirmDialog
          title="Revoke display token"
          confirmLabel="Revoke token"
          run={() => api.send<void>('DELETE', `/displays/${seg(dialog.display.id)}/token`)}
          onDone={() => {
            setDialog(undefined);
            displays.reload();
          }}
          onClose={() => {
            setDialog(undefined);
          }}
        >
          {`Revoke the token of "${dialog.display.name}"? The display can no longer load its dashboards until it gets a new token.`}
        </ConfirmDialog>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- Page

export function ConfigurationPage() {
  const { can } = useSession();
  const { status } = useStatus();
  const now = useNow();
  // The one place a key or token lives: set from an answer, cleared when its dialog closes.
  const [secret, setSecret] = useState<Secret | undefined>(undefined);

  return (
    <>
      <h1>Configuration</h1>
      <SyncSection status={status} now={now} />
      {status?.gitops === true && <HistorySection now={now} />}
      {can('operator') && <UnknownAgentsSection now={now} />}
      {can('admin') && <KeysSection now={now} onSecret={setSecret} />}
      {can('admin') && <DisplaysSection now={now} onSecret={setSecret} />}
      {secret !== undefined && (
        <SecretDialog
          title={secret.title}
          secret={secret.value}
          note={secret.note}
          onClose={() => {
            setSecret(undefined);
          }}
        />
      )}
    </>
  );
}
