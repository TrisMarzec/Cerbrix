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
