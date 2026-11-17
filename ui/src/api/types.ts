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

/** One row of `GET /hosts`. */
export interface HostRow {
  name: string;
  address: string | null;
  groups: string[];
  /** `null` without open problems. */
  state: HostState | null;
  last_seen_at: string | null;
  managed_by_git: boolean;
  /** Newest `cpu.usage` of the last ten minutes. */
  cpu: number | null;
  /** Newest `memory.used_percent` of the last ten minutes. */
  memory: number | null;
}

/** `GET /hosts`: a page of hosts, the number of all matching hosts, and whether the values could be read. */
export interface HostsAnswer extends Listed<HostRow> {
  metrics_available: boolean;
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

/** One item of the effective configuration of a host (`GET /hosts/{name}`). */
export interface HostItem {
  key: string;
  /** The instance the item is narrowed to, or `null`. */
  params: Record<string, string> | null;
  interval_secs: number;
  /** `host` or `template:<name>`. */
  origin: string;
}

/** One trigger of the effective configuration of a host. */
export interface HostTrigger {
  name: string;
  expression: string;
  severity: Severity;
  /** `host` or `template:<name>`. */
  origin: string;
  firing: boolean;
}

/** `GET /hosts/{name}`: the host with its effective configuration and its open problems. */
export interface HostDetail {
  name: string;
  address: string | null;
  groups: string[];
  templates: string[];
  /** The file that defines the host; `null` for a host created by hand. */
  source_file: string | null;
  managed_by_git: boolean;
  enrolled_at: string | null;
  last_seen_at: string | null;
  agent_version: string | null;
  dashboards: { id: string; name: string }[];
  state: HostState | null;
  items: HostItem[];
  triggers: HostTrigger[];
  open_problems: Problem[];
}

/** One entry of `GET /hosts/{name}/values`: the newest value of one series. */
export interface HostValue {
  key: string;
  params: Record<string, string>;
  value: number;
  at: string;
}

/** `GET /hosts/{name}/values`. */
export interface HostValuesAnswer {
  items: HostValue[];
  metrics_available: boolean;
}

/** The limit of a comparison trigger, drawn into a graph. */
export interface Threshold {
  trigger: string;
  severity: Severity;
  op: string;
  value: number;
}

/** `GET /hosts/{name}/series`: the history of one series. Points are `[timestamp_ms, value]`. */
export interface Series {
  key: string;
  params: Record<string, string>;
  from: string;
  to: string;
  step_secs: number;
  points: [number, number][];
  thresholds: Threshold[];
}

/** `GET /problems`: a page of problems, the number of all matching problems, and the dashboards of each host on the page. */
export interface ProblemsAnswer extends Listed<Problem> {
  /** For every host in `items`: the dashboards that show it (sorted by name); empty for a host in none. */
  dashboards: Record<string, { id: string; name: string }[]>;
}

/** The names of one kind of configuration object in one applied revision. */
export interface ChangeLists {
  added: string[];
  changed: string[];
  removed: string[];
}

/** What a revision changed (`GET /sync/history`). */
export interface SyncChanges {
  hosts: ChangeLists;
  templates: ChangeLists;
  notifications: ChangeLists;
  dashboards: ChangeLists;
  displays: ChangeLists;
}

/** One applied revision of the configuration repository. */
export interface SyncHistoryEntry {
  commit: string;
  applied_at: string;
  changes: SyncChanges;
}

/** An agent that tried to enrol under a name that is not a registered host (`GET /unknown-agents`). */
export interface UnknownAgent {
  host_name: string;
  first_seen: string;
  last_seen: string;
  attempts: number;
}

export type EnrollmentKeyState = 'active' | 'expired' | 'revoked';

/** An enrolment key without the key itself (`GET /enrollment-keys`). */
export interface EnrollmentKey {
  id: string;
  description: string;
  created_at: string;
  expires_at: string;
  revoked_at: string | null;
  state: EnrollmentKeyState;
}
