import './AdministrationPage.css';

import { useEffect, useRef, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { Listed, Role, User } from '../api/types';
import { ActionDialog } from '../components/ActionDialog';
import { Banner } from '../components/Banner';
import { BusyRegion } from '../components/BusyRegion';
import { describeError, hasStatus } from '../components/describeError';
import { Empty } from '../components/Empty';
import { Pager } from '../components/Pager';
import { SecretDialog } from '../components/SecretDialog';
import { StaleNote } from '../components/StaleNote';
import { TimeText } from '../components/TimeText';
import { roleWord } from '../lib/format';
import { usePaging } from '../lib/paging';
import { usePolled } from '../lib/poll';
import { useSession } from '../session';
import { useNow } from '../shell/context';

const LIMIT = 50;
const NAME_PATTERN = /^[a-z0-9][a-z0-9._-]{0,62}$/;
const PASSWORD_MIN = 12;
const PASSWORD_MAX = 256;

const NAME_PROBLEM =
  'Use 1 to 63 characters: a–z, 0–9, dot, underscore, hyphen; start with a letter or digit.';
const GONE = 'This user no longer exists.';
const OWN_SESSION = 'This ends your own session.';
const PASSWORD_NOTE = 'Give this password to the user. They should change it after the first login.';

const ROLES: Role[] = ['viewer', 'operator', 'admin'];

function isRole(value: string): value is Role {
  return ROLES.some((r) => r === value);
}

/** True until the component unmounts; a late answer then sets no state. */
function useMounted() {
  const mounted = useRef(true);
  useEffect(
    () => () => {
      mounted.current = false;
    },
    [],
  );
  return mounted;
}

/** A request about one user answers 404 when the user was removed meanwhile. */
const ROW_TEXTS = { 404: GONE };
/** Creating a user answers 409 when the name is taken. */
const CREATE_TEXTS = { 409: 'This name is taken.' };


/** A secret shown once; the page holds it only while its dialog is open. */
interface Secret {
  title: string;
  value: string;
}

// ---------------------------------------------------------------- Dialogs

/**
 * What a dialog about one user does on close: it asks for a reload of the list when one of its
 * requests learned that the user is gone.
 */
function useGone(onClose: Closed): { onFailed: (e: unknown) => void; onClose: () => void } {
  const gone = useRef(false);
  return {
    onFailed: (e) => {
      if (hasStatus(e, 404)) gone.current = true;
    },
    onClose: () => {
      onClose(gone.current);
    },
  };
}

function OwnSessionWarning({ own }: { own: boolean }) {
  return own ? <p>{OWN_SESSION}</p> : null;
}

// ---------------------------------------------------------------- Password choice

interface PasswordChoice {
  generate: boolean;
  setGenerate: (generate: boolean) => void;
  password: string;
  setPassword: (text: string) => void;
  again: string;
  setAgain: (text: string) => void;
}

function usePasswordChoice(): PasswordChoice {
  const [generate, setGenerate] = useState(true);
  const [password, setPassword] = useState('');
  const [again, setAgain] = useState('');
  return { generate, setGenerate, password, setPassword, again, setAgain };
}

function passwordProblem(choice: PasswordChoice): string | undefined {
  if (choice.generate) return undefined;
  const length = Array.from(choice.password).length;
  if (length < PASSWORD_MIN || length > PASSWORD_MAX) {
    return `Use ${PASSWORD_MIN} to ${PASSWORD_MAX} characters for the password.`;
  }
  if (choice.password !== choice.again) return 'The two passwords are not equal.';
  return undefined;
}

function clearPasswords(choice: PasswordChoice) {
  choice.setPassword('');
  choice.setAgain('');
}

function PasswordFields({ choice }: { choice: PasswordChoice }) {
  return (
    <fieldset class="admin-choice">
      <legend>How to set the password</legend>
      <label class="admin-radio">
        <input
          type="radio"
          name="password-mode"
          checked={choice.generate}
          onChange={() => {
            choice.setGenerate(true);
          }}
        />
        Generate a password
      </label>
      <label class="admin-radio">
        <input
          type="radio"
          name="password-mode"
          checked={!choice.generate}
          onChange={() => {
            choice.setGenerate(false);
          }}
        />
        Set a password
      </label>
      {!choice.generate && (
        <>
          <label>
            Password
            <input
              type="password"
              autocomplete="new-password"
              value={choice.password}
              onInput={(e) => {
                choice.setPassword(e.currentTarget.value);
              }}
            />
          </label>
          <label>
            Repeat the password
            <input
              type="password"
              autocomplete="new-password"
              value={choice.again}
              onInput={(e) => {
                choice.setAgain(e.currentTarget.value);
              }}
            />
          </label>
        </>
      )}
    </fieldset>
  );
}

type Generated = { password?: string };
type Closed = (reload: boolean) => void;

function RoleSelect({ value, onChange }: { value: Role; onChange: (role: Role) => void }) {
  return (
    <label>
      Role
      <select
        value={value}
        onChange={(e) => {
          const next = e.currentTarget.value;
          if (isRole(next)) onChange(next);
        }}
      >
        {ROLES.map((r) => (
          <option key={r} value={r}>
            {roleWord(r)}
          </option>
        ))}
      </select>
    </label>
  );
}

function AddUserDialog({
  onDone,
  onClose,
}: {
  onDone: (created: User & Generated) => void;
  onClose: Closed;
}) {
  const [name, setName] = useState('');
  const [role, setRole] = useState<Role>('viewer');
  const choice = usePasswordChoice();

  return (
    <ActionDialog
      title="Add user"
      submitLabel="Add user"
      texts={CREATE_TEXTS}
      validate={() => (NAME_PATTERN.test(name) ? passwordProblem(choice) : NAME_PROBLEM)}
      run={() =>
        api.send<User & Generated>('POST', '/users', {
          name,
          role,
          ...(choice.generate ? {} : { password: choice.password }),
        })
      }
      afterAttempt={() => {
        clearPasswords(choice);
      }}
      onDone={onDone}
      onClose={() => {
        onClose(false);
      }}
    >
      <label>
        Name
        <input
          type="text"
          autocomplete="off"
          value={name}
          onInput={(e) => {
            setName(e.currentTarget.value);
          }}
        />
      </label>
      <RoleSelect value={role} onChange={setRole} />
      <PasswordFields choice={choice} />
    </ActionDialog>
  );
}

interface RowDialogProps {
  user: User;
  own: boolean;
  onDone: () => void;
  onClose: Closed;
}

function RoleDialog({ user, own, onDone, onClose }: RowDialogProps) {
  const [role, setRole] = useState<Role>(user.role);
  return (
    <ActionDialog
      title="Change role"
      submitLabel="Change role"
      submitDisabled={role === user.role}
      run={() => api.send<User>('PATCH', `/users/${seg(user.id)}`, { role })}
      texts={ROW_TEXTS}
      onDone={onDone}
      {...useGone(onClose)}
    >
      <RoleSelect value={role} onChange={setRole} />
      <OwnSessionWarning own={own} />
    </ActionDialog>
  );
}

function DisableDialog({ user, own, onDone, onClose }: RowDialogProps) {
  return (
    <ActionDialog
      title="Disable user"
      submitLabel="Disable user"
      run={() => api.send<User>('PATCH', `/users/${seg(user.id)}`, { disabled: true })}
      texts={ROW_TEXTS}
      onDone={onDone}
      {...useGone(onClose)}
    >
      <p>{`Disable ${user.name}? They are logged out at once and cannot log in until enabled.`}</p>
      <OwnSessionWarning own={own} />
    </ActionDialog>
  );
}

function RemoveDialog({ user, own, onDone, onClose }: RowDialogProps) {
  return (
    <ActionDialog
      title="Remove user"
      submitLabel="Remove user"
      run={() => api.send<void>('DELETE', `/users/${seg(user.id)}`)}
      texts={ROW_TEXTS}
      onDone={onDone}
      {...useGone(onClose)}
    >
      <p>{`Remove ${user.name}? This cannot be undone.`}</p>
      <OwnSessionWarning own={own} />
    </ActionDialog>
  );
}

/** Never opened for the own account: the server would end the session that shows the new password. */
function ResetDialog({
  user,
  onDone,
  onClose,
}: {
  user: User;
  onDone: (answer: Generated) => void;
  onClose: Closed;
}) {
  const choice = usePasswordChoice();
  return (
    <ActionDialog
      title="Reset password"
      submitLabel="Reset password"
      validate={() => passwordProblem(choice)}
      run={() =>
        api.send<Generated>(
          'POST',
          `/users/${seg(user.id)}/password`,
          choice.generate ? {} : { password: choice.password },
        )
      }
      afterAttempt={() => {
        clearPasswords(choice);
      }}
      texts={ROW_TEXTS}
      onDone={onDone}
      {...useGone(onClose)}
    >
      <p>{`All sessions of ${user.name} end.`}</p>
      <PasswordFields choice={choice} />
    </ActionDialog>
  );
}

// ---------------------------------------------------------------- Page

type RowKind = 'role' | 'disable' | 'reset' | 'remove';
type Open = { kind: 'add' } | { kind: RowKind; user: User } | undefined;

export function AdministrationPage() {
  const { user: me } = useSession();
  const now = useNow();
  const mounted = useMounted();
  const paging = usePaging('offset', LIMIT);
  const offset = paging.offset;

  const users = usePolled(
    (signal) => api.get<Listed<User>>('/users', { limit: LIMIT, offset }, signal),
    [offset],
  );
  const [open, setOpen] = useState<Open>(undefined);
  // The one place a generated password lives: set from an answer, cleared when its dialog closes.
  const [secret, setSecret] = useState<Secret | undefined>(undefined);
  const [enabling, setEnabling] = useState<string | undefined>(undefined);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const answer = users.data;
  // Fewer users than the offset: the last page that exists replaces it (once the answer is in).
  const { beyond, shownOffset } = paging.correct(answer?.total, users.pending);

  // Another page: the failure of an Enable on the old one no longer applies.
  useEffect(() => {
    setFailure(undefined);
  }, [offset]);

  const failed = answer === undefined && users.error !== undefined;

  const closeDialog: Closed = (reload) => {
    setOpen(undefined);
    if (reload) users.reload();
  };
  const done = () => {
    setOpen(undefined);
    setFailure(undefined);
    users.reload();
  };
  const gotPassword = (title: string, answered: Generated) => {
    done();
    if (typeof answered.password === 'string') setSecret({ title, value: answered.password });
  };

  const enable = (user: User) => {
    if (enabling !== undefined) return;
    setEnabling(user.id);
    setFailure(undefined);
    api.send<User>('PATCH', `/users/${seg(user.id)}`, { disabled: false }).then(
      () => {
        if (!mounted.current) return;
        setEnabling(undefined);
        setFailure(undefined);
        users.reload();
      },
      (e: unknown) => {
        if (!mounted.current) return;
        setFailure(describeError(e, ROW_TEXTS));
        setEnabling(undefined);
        if (hasStatus(e, 404)) users.reload();
      },
    );
  };

  const rowButton = (user: User, kind: RowKind, label: string, aria: string) => (
    <button
      type="button"
      class="btn"
      aria-label={aria}
      disabled={enabling === user.id}
      onClick={() => {
        setOpen({ kind, user });
      }}
    >
      {label}
    </button>
  );

  return (
    <>
      <h1>Administration</h1>
      <section class="admin-section">
        <h2>Users</h2>
        {failed && (
          <Banner kind="warning">
            Could not load the users.{' '}
            <button type="button" class="btn" onClick={users.reload}>
              Try again
            </button>
          </Banner>
        )}
        <div class="admin-toolbar">
          <button
            type="button"
            class="btn"
            onClick={() => {
              setOpen({ kind: 'add' });
            }}
          >
            Add user
          </button>
        </div>
        {failure !== undefined && (
          <p role="alert" class="form-error">
            {failure}
          </p>
        )}
        {answer !== undefined && (
          // Everything below comes from the last answer: while the answer for a new page is
          // outstanding it is dimmed and marked busy.
          <BusyRegion pending={users.pending} class="admin-result">
            <StaleNote of={users} />
            {answer.total === 0 ? (
              !users.pending && <Empty>No users.</Empty>
            ) : beyond ? null : (
              <>
                {answer.items.length > 0 ? (
                  <div class="table-scroll">
                    <table class="admin-table" aria-label="Users">
                      <thead>
                        <tr>
                          <th scope="col">Name</th>
                          <th scope="col">Role</th>
                          <th scope="col">State</th>
                          <th scope="col">Created</th>
                          <th scope="col">Password changed</th>
                          <th scope="col">
                            <span class="visually-hidden">Actions</span>
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {answer.items.map((u) => {
                          const own = u.name === me.name;
                          return (
                            <tr key={u.id} class={u.disabled ? 'admin-dim-row' : undefined}>
                              <th scope="row">
                                <span class="admin-name">{u.name}</span>
                                {own && <span class="admin-you">you</span>}
                              </th>
                              <td>{roleWord(u.role)}</td>
                              <td>{u.disabled ? 'Disabled' : 'Active'}</td>
                              <td>
                                <TimeText iso={u.created_at} now={now} />
                              </td>
                              <td>
                                <TimeText iso={u.password_changed_at} now={now} />
                              </td>
                              <td class="admin-actions">
                                {rowButton(u, 'role', 'Change role', `Change role of ${u.name}`)}
                                {u.disabled ? (
                                  <button
                                    type="button"
                                    class="btn"
                                    aria-label={`Enable ${u.name}`}
                                    disabled={enabling !== undefined}
                                    onClick={() => {
                                      enable(u);
                                    }}
                                  >
                                    Enable
                                  </button>
                                ) : (
                                  rowButton(u, 'disable', 'Disable', `Disable ${u.name}`)
                                )}
                                {own ? (
                                  <span class="dim admin-hint">
                                    Use Change password for your own account.
                                  </span>
                                ) : (
                                  rowButton(u, 'reset', 'Reset password', `Reset password of ${u.name}`)
                                )}
                                {rowButton(u, 'remove', 'Remove', `Remove ${u.name}`)}
                              </td>
                            </tr>
                          );
                        })}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  !users.pending && <Empty>Nothing on this page.</Empty>
                )}
                <Pager
                  total={answer.total}
                  limit={LIMIT}
                  offset={shownOffset}
                  disabled={users.pending}
                  onChange={paging.set}
                />
              </>
            )}
          </BusyRegion>
        )}
      </section>
      {open?.kind === 'add' && (
        <AddUserDialog
          onDone={(created) => {
            gotPassword(`Password of ${created.name}`, created);
          }}
          onClose={closeDialog}
        />
      )}
      {open !== undefined && open.kind !== 'add' && (
        <RowDialog open={open} me={me.name} onDone={done} onPassword={gotPassword} onClose={closeDialog} />
      )}
      {secret !== undefined && (
        <SecretDialog
          title={secret.title}
          secret={secret.value}
          note={PASSWORD_NOTE}
          onClose={() => {
            setSecret(undefined);
          }}
        />
      )}
    </>
  );
}

interface RowDialogHostProps {
  open: { kind: RowKind; user: User };
  me: string;
  onDone: () => void;
  onPassword: (title: string, answer: Generated) => void;
  onClose: Closed;
}

function RowDialog({ open, me, onDone, onPassword, onClose }: RowDialogHostProps) {
  const { user } = open;
  const own = user.name === me;
  switch (open.kind) {
    case 'role':
      return <RoleDialog user={user} own={own} onDone={onDone} onClose={onClose} />;
    case 'disable':
      return <DisableDialog user={user} own={own} onDone={onDone} onClose={onClose} />;
    case 'remove':
      return <RemoveDialog user={user} own={own} onDone={onDone} onClose={onClose} />;
    case 'reset':
      return (
        <ResetDialog
          user={user}
          onDone={(answer) => {
            onPassword(`Password of ${user.name}`, answer);
          }}
          onClose={onClose}
        />
      );
  }
}
