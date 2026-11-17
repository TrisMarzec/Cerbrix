export type Severity = 'info' | 'warning' | 'high' | 'critical';

export type ProblemKind = 'compare' | 'no_data';

export type ResolveReason =
  | 'condition_ended'
  | 'host_removed'
  | 'trigger_removed'
  | 'closed_by_operator';

/** A problem, in the form every route of the API returns it (docs/api.md, "Problems"). */
export interface Problem {
  id: string;
  host: string;
  trigger: string;
  instance: string;
  severity: Severity;
  kind: ProblemKind;
  summary: string;
  started_at: string;
  resolved_at: string | null;
  resolve_reason: ResolveReason | null;
  acknowledged_at: string | null;
  acknowledged_by: string | null;
  ack_comment: string | null;
  closed_by: string | null;
  close_comment: string | null;
}

/** The state of a host: the problem that decides, plus the number of open problems. */
export interface HostState {
  severity: Severity;
  acknowledged: boolean;
  kind: ProblemKind;
  trigger: string;
  instance: string;
  summary: string;
  open_problems: number;
}

export type Role = 'viewer' | 'operator' | 'admin';

/** The logged-in user (`GET /session`, `POST /session`). */
export interface SessionUser {
  name: string;
  role: Role;
}

export type SyncStatus = 'never' | 'in_sync' | 'out_of_sync';

/** Why the last synchronisation failed; everything but `reason` can be null. */
export interface SyncIssue {
  file: string | null;
  line: number | null;
  column: number | null;
  reason: string;
}

export interface SyncState {
  status: SyncStatus;
  active_commit: string | null;
  last_success_at: string | null;
  last_attempt_at: string | null;
  issues: SyncIssue[];
}

export interface ProblemCount {
  open: number;
  unacknowledged: number;
}

/** `GET /status`: what the header of every page needs. */
export interface Status {
  version: string;
  gitops: boolean;
  mail: boolean;
  sync: SyncState;
  hosts: {
    total: number;
    with_problems: number;
    without_problems: number;
    never_seen: number;
  };
  problems: Record<Severity, ProblemCount>;
}

/** A paged list (`limit`/`offset`): the page and the number of all matching items. */
export interface Listed<T> {
  items: T[];
  total: number;
}
