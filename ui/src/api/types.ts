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

/** How many hosts of a dashboard are in each state (`GET /dashboards`). */
export interface HostsByState {
  critical: number;
  high: number;
  warning: number;
  info: number;
  acknowledged: number;
  ok: number;
}

/** One entry of `GET /dashboards`. */
export interface DashboardSummary {
  id: string;
  name: string;
  file: string;
  priority: number;
  host_count: number;
  hosts_by_state: HostsByState;
  open_problems: Record<Severity, number>;
  unacknowledged: Record<Severity, number>;
}

export type DisplayShow = { dashboard: string } | { rotate: string[]; every: string };

/** One entry of `GET /displays`. */
export interface Display {
  id: string;
  name: string;
  file: string;
  show: DisplayShow;
  switch_on_alarm: { min_severity: Severity } | null;
  dashboards: string[] | null;
  token_created_at: string | null;
  target: string | null;
  causes: string[];
  waiting: string[];
}
