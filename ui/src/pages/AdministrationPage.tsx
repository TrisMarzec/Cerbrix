import "./AdministrationPage.css";

import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";

import { api, ApiError, seg } from "../api/client";
import type { Listed, Role, User } from "../api/types";
import { Banner } from "../components/Banner";
import { Dialog } from "../components/Dialog";
import { Empty } from "../components/Empty";
import { Pager } from "../components/Pager";
import { SecretDialog } from "../components/SecretDialog";
import { TimeText } from "../components/TimeText";
import { usePolled } from "../lib/poll";
import { useQueryState } from "../lib/query";
import { useSession } from "../session";
import { useNow } from "../shell/context";

const LIMIT = 50;
const NAME_PATTERN = /^[a-z0-9][a-z0-9._-]{0,62}$/;
const PASSWORD_MIN = 12;
const PASSWORD_MAX = 256;

const NAME_PROBLEM =
  "Use 1 to 63 characters: a–z, 0–9, dot, underscore, hyphen; start with a letter or digit.";
const GONE = "This user no longer exists.";
const OWN_SESSION = "This ends your own session.";
const PASSWORD_NOTE =
  "Give this password to the user. They should change it after the first login.";

const ROLES: { value: Role; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "operator", label: "Operator" },
  { value: "admin", label: "Admin" },
];

function roleWord(role: Role): string {
  return ROLES.find((r) => r.value === role)?.label ?? role;
}

function isRole(value: string): value is Role {
  return ROLES.some((r) => r.value === value);
}

function parseOffset(text: string | null): number {
  if (text === null || !/^\d+$/.test(text)) return 0;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : 0;
}

function offsetParam(offset: number): string | undefined {
  return offset === 0 ? undefined : String(offset);
}

/** The sentence for a failed request, and whether it means the user is gone. */
function explain(
  e: unknown,
  onCreate = false,
): { text: string; gone: boolean } {
  if (!(e instanceof ApiError) || e.status === 0) {
    return { text: "Could not reach the server.", gone: false };
  }
  switch (e.status) {
    case 403:
      return { text: "Your role may not do this.", gone: false };
    case 404:
      return { text: GONE, gone: true };
    case 409:
      return {
        text: onCreate ? "This name is taken." : e.message,
        gone: false,
      };
    case 429:
      return {
        text: "The server is busy, try again in a moment.",
        gone: false,
      };
    default:
      return { text: e.message, gone: false };
  }
}

/** A secret shown once; the page holds it only while its dialog is open. */
interface Secret {
  title: string;
  value: string;
}

// ---------------------------------------------------------------- Dialog frame

interface FrameProps<T> {
  title: string;
  submitLabel: string;
  submitDisabled?: boolean;
  children: ComponentChildren;
  /** A sentence when the input is not acceptable; no request is sent then. */
  validate?: () => string | undefined;
  run: () => Promise<T>;
  /** Called after every attempt that reached the server or failed, for example to empty password fields. */
  afterAttempt?: () => void;
  /** The failure of a create is read differently (`409` means the name is taken). */
  onCreate?: boolean;
  onDone: (result: T) => void;
  /** `reload` is true when the dialog learned that the list is out of date. */
  onClose: (reload: boolean) => void;
}

/** A form dialog that runs one request at a time, stays open on failure and cannot be dismissed meanwhile. */
function Frame<T>({
  title,
  submitLabel,
  submitDisabled = false,
  children,
  validate,
  run,
  afterAttempt,
  onCreate = false,
  onDone,
  onClose,
}: FrameProps<T>) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [stale, setStale] = useState(false);

  const close = () => {
    if (!busy) onClose(stale);
  };

  const submit = (e: Event) => {
    e.preventDefault();
    if (busy) return;
    const problem = validate?.();
    if (problem !== undefined) {
      setFailure(problem);
      return;
    }
    setBusy(true);
    setFailure(undefined);
    run().then(
      (result) => {
        afterAttempt?.();
        onDone(result);
      },
      (error: unknown) => {
        const { text, gone } = explain(error, onCreate);
        afterAttempt?.();
        setFailure(text);
        if (gone) setStale(true);
        setBusy(false);
      },
    );
  };

  return (
    <Dialog title={title} closeOnBackdrop={!busy} onClose={close}>
      <form class="admin-form" onSubmit={submit} noValidate>
        {children}
        {failure !== undefined && (
          <p role="alert" class="admin-error">
            {failure}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" disabled={busy} onClick={close}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy || submitDisabled}>
            {submitLabel}
          </button>
        </div>
      </form>
    </Dialog>
  );
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
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  return { generate, setGenerate, password, setPassword, again, setAgain };
}

function passwordProblem(choice: PasswordChoice): string | undefined {
  if (choice.generate) return undefined;
  const length = Array.from(choice.password).length;
  if (length < PASSWORD_MIN || length > PASSWORD_MAX) {
    return `Use ${PASSWORD_MIN} to ${PASSWORD_MAX} characters for the password.`;
  }
  if (choice.password !== choice.again)
    return "The two passwords are not equal.";
  return undefined;
}

function clearPasswords(choice: PasswordChoice) {
  choice.setPassword("");
  choice.setAgain("");
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
              autoComplete="new-password"
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
              autoComplete="new-password"
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

// ---------------------------------------------------------------- Dialogs

type Generated = { password?: string };
type Closed = (reload: boolean) => void;

function AddUserDialog({
  onDone,
  onClose,
}: {
  onDone: (created: User & Generated) => void;
  onClose: Closed;
}) {
  const [name, setName] = useState("");
  const [role, setRole] = useState<Role>("viewer");
  const choice = usePasswordChoice();

  return (
    <Frame
      title="Add user"
      submitLabel="Add user"
      onCreate
      validate={() =>
        NAME_PATTERN.test(name) ? passwordProblem(choice) : NAME_PROBLEM
      }
      run={() =>
        api.send<User & Generated>("POST", "/users", {
          name,
          role,
          ...(choice.generate ? {} : { password: choice.password }),
        })
      }
      afterAttempt={() => {
        clearPasswords(choice);
      }}
      onDone={onDone}
      onClose={onClose}
    >
      <label>
        Name
        <input
          type="text"
          autoComplete="off"
          value={name}
          onInput={(e) => {
            setName(e.currentTarget.value);
          }}
        />
      </label>
      <RoleSelect value={role} onChange={setRole} />
      <PasswordFields choice={choice} />
    </Frame>
  );
}

function RoleSelect({
  value,
  onChange,
}: {
  value: Role;
  onChange: (role: Role) => void;
}) {
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
          <option key={r.value} value={r.value}>
            {r.label}
          </option>
        ))}
      </select>
    </label>
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
    <Frame
      title="Change role"
      submitLabel="Change role"
      submitDisabled={role === user.role}
      run={() => api.send<User>("PATCH", `/users/${seg(user.id)}`, { role })}
      onDone={onDone}
      onClose={onClose}
    >
      <RoleSelect value={role} onChange={setRole} />
      <OwnSessionWarning own={own} />
    </Frame>
  );
}

function DisableDialog({ user, own, onDone, onClose }: RowDialogProps) {
  return (
    <Frame
      title="Disable user"
      submitLabel="Disable user"
      run={() =>
        api.send<User>("PATCH", `/users/${seg(user.id)}`, { disabled: true })
      }
      onDone={onDone}
      onClose={onClose}
    >
      <p>{`Disable ${user.name}? They are logged out at once and cannot log in until enabled.`}</p>
      <OwnSessionWarning own={own} />
    </Frame>
  );
}

function RemoveDialog({ user, own, onDone, onClose }: RowDialogProps) {
  return (
    <Frame
      title="Remove user"
      submitLabel="Remove user"
      run={() => api.send<void>("DELETE", `/users/${seg(user.id)}`)}
      onDone={onDone}
      onClose={onClose}
    >
      <p>{`Remove ${user.name}? This cannot be undone.`}</p>
      <OwnSessionWarning own={own} />
    </Frame>
  );
}

function ResetDialog({
  user,
  own,
  onDone,
  onClose,
}: Omit<RowDialogProps, "onDone"> & { onDone: (answer: Generated) => void }) {
  const choice = usePasswordChoice();
  return (
    <Frame
      title="Reset password"
      submitLabel="Reset password"
      validate={() => passwordProblem(choice)}
      run={() =>
        api.send<Generated>(
          "POST",
          `/users/${seg(user.id)}/password`,
          choice.generate ? {} : { password: choice.password },
        )
      }
      afterAttempt={() => {
        clearPasswords(choice);
      }}
      onDone={onDone}
      onClose={onClose}
    >
      <p>{`All sessions of ${user.name} end.`}</p>
      <OwnSessionWarning own={own} />
      <PasswordFields choice={choice} />
    </Frame>
  );
}

// ---------------------------------------------------------------- Page

type Open =
  | { kind: "add" }
  | { kind: "role" | "disable" | "reset" | "remove"; user: User }
  | undefined;

export function AdministrationPage() {
  const { user: me } = useSession();
  const now = useNow();
  const [query, setQuery] = useQueryState();
  const offset = parseOffset(query.get("offset"));

  const users = usePolled(
    (signal) =>
      api.get<Listed<User>>("/users", { limit: LIMIT, offset }, signal),
    [offset],
  );
  const [open, setOpen] = useState<Open>(undefined);
  // The one place a generated password lives: set from an answer, cleared when its dialog closes.
  const [secret, setSecret] = useState<Secret | undefined>(undefined);
  const [enabling, setEnabling] = useState<string | undefined>(undefined);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const answer = users.data;
  const total = answer?.total ?? 0;
  const beyond = answer !== undefined && total > 0 && total <= offset;

  // Fewer users than the offset: go back to the last page that exists (once; the next answer is in range).
  useEffect(() => {
    if (!users.pending && beyond) {
      setQuery({
        offset: offsetParam(Math.floor((total - 1) / LIMIT) * LIMIT),
      });
    }
  }, [users.pending, beyond, total]);

  const failed = answer === undefined && users.error !== undefined;

  const closeDialog: Closed = (reload) => {
    setOpen(undefined);
    if (reload) users.reload();
  };
  const done = () => {
    setOpen(undefined);
    users.reload();
  };
  const gotPassword = (title: string, answered: Generated) => {
    done();
    if (typeof answered.password === "string")
      setSecret({ title, value: answered.password });
  };

  const enable = (user: User) => {
    if (enabling !== undefined) return;
    setEnabling(user.id);
    setFailure(undefined);
    api.send<User>("PATCH", `/users/${seg(user.id)}`, { disabled: false }).then(
      () => {
        setEnabling(undefined);
        users.reload();
      },
      (e: unknown) => {
        const { text, gone } = explain(e);
        setFailure(text);
        setEnabling(undefined);
        if (gone) users.reload();
      },
    );
  };

  const button = (
    user: User,
    kind: "role" | "disable" | "reset" | "remove",
    label: string,
    what: string,
  ) => (
    <button
      type="button"
      class="btn"
      aria-label={`${label}${what} ${user.name}`}
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
            Could not load the users.{" "}
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
              setOpen({ kind: "add" });
            }}
          >
            Add user
          </button>
        </div>
        {failure !== undefined && (
          <p role="alert" class="admin-error">
            {failure}
          </p>
        )}
        {answer !== undefined && (
          // Everything below comes from the last answer: while the answer for a new page is
          // outstanding it is dimmed and marked busy.
          <div
            class={users.pending ? "admin-result is-busy" : "admin-result"}
            aria-busy={users.pending}
          >
            {answer.total === 0 ? (
              !users.pending && <Empty>No users.</Empty>
            ) : beyond ? null : (
              <>
                {answer.items.length > 0 ? (
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
                      {answer.items.map((u) => (
                        <tr
                          key={u.id}
                          class={u.disabled ? "admin-dim-row" : undefined}
                        >
                          <th scope="row">
                            <span class="admin-name">{u.name}</span>
                            {u.name === me.name && (
                              <span class="admin-you">you</span>
                            )}
                          </th>
                          <td>{roleWord(u.role)}</td>
                          <td>{u.disabled ? "Disabled" : "Active"}</td>
                          <td>
                            <TimeText iso={u.created_at} now={now} />
                          </td>
                          <td>
                            <TimeText iso={u.password_changed_at} now={now} />
                          </td>
                          <td class="admin-actions">
                            {button(u, "role", "Change role", " of")}
                            {u.disabled ? (
                              <button
                                type="button"
                                class="btn"
                                aria-label={`Enable ${u.name}`}
                                disabled={enabling === u.id}
                                onClick={() => {
                                  enable(u);
                                }}
                              >
                                Enable
                              </button>
                            ) : (
                              button(u, "disable", "Disable", "")
                            )}
                            {button(u, "reset", "Reset password", " of")}
                            {button(u, "remove", "Remove", "")}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                ) : (
                  !users.pending && <Empty>Nothing on this page.</Empty>
                )}
                <Pager
                  total={answer.total}
                  limit={LIMIT}
                  offset={offset}
                  onChange={(next) => {
                    setQuery({ offset: offsetParam(next) });
                  }}
                />
              </>
            )}
          </div>
        )}
      </section>
      {open?.kind === "add" && (
        <AddUserDialog
          onDone={(created) => {
            gotPassword(`Password of ${created.name}`, created);
          }}
          onClose={closeDialog}
        />
      )}
      {open !== undefined && open.kind !== "add" && (
        <RowDialog
          open={open}
          me={me.name}
          onDone={gotPassword}
          onClose={closeDialog}
          done={done}
        />
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
  open: { kind: "role" | "disable" | "reset" | "remove"; user: User };
  me: string;
  done: () => void;
  onDone: (title: string, answer: Generated) => void;
  onClose: Closed;
}

function RowDialog({ open, me, done, onDone, onClose }: RowDialogHostProps) {
  const { user } = open;
  const own = user.name === me;
  switch (open.kind) {
    case "role":
      return (
        <RoleDialog user={user} own={own} onDone={done} onClose={onClose} />
      );
    case "disable":
      return (
        <DisableDialog user={user} own={own} onDone={done} onClose={onClose} />
      );
    case "remove":
      return (
        <RemoveDialog user={user} own={own} onDone={done} onClose={onClose} />
      );
    case "reset":
      return (
        <ResetDialog
          user={user}
          own={own}
          onDone={(answer) => {
            onDone(`Password of ${user.name}`, answer);
          }}
          onClose={onClose}
        />
      );
  }
}
