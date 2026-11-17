import type { HostState, Problem } from '../api/types';

/** The fixed "now" of the tests: 17 Nov 2026, 14:30 local time. */
export const NOW = new Date(2026, 10, 17, 14, 30, 0);

function local(y: number, mo: number, d: number, h: number, mi: number, s = 0): string {
  return new Date(y, mo, d, h, mi, s).toISOString();
}

export const criticalProblem: Problem = {
  id: '0c208a84-6643-46d4-86ab-066f04916ecc',
  host: 'db-01',
  trigger: 'Disk almost full',
  instance: 'mount=/',
  severity: 'critical',
  kind: 'compare',
  summary: 'disk.used_percent > 90: value 94.2',
  started_at: local(2026, 10, 17, 12, 16),
  resolved_at: null,
  resolve_reason: null,
  acknowledged_at: null,
  acknowledged_by: null,
  ack_comment: null,
  closed_by: null,
  close_comment: null,
};

export const acknowledgedProblem: Problem = {
  id: '6fa4ff3a-6224-462b-a96f-ab6f3daca0f1',
  host: 'web-02',
  trigger: 'Memory high',
  instance: '',
  severity: 'warning',
  kind: 'compare',
  summary: 'memory.used_percent >= 80: value 85.1',
  started_at: local(2026, 10, 16, 9, 0),
  resolved_at: null,
  resolve_reason: null,
  acknowledged_at: local(2026, 10, 17, 14, 5),
  acknowledged_by: 'anna',
  ack_comment: 'watching',
  closed_by: null,
  close_comment: null,
};

export const resolvedProblem: Problem = {
  id: 'b1f5c1b2-2d6f-4a0e-9a35-0d6d8a8d3f11',
  host: 'web-01',
  trigger: 'Agent silent',
  instance: '',
  severity: 'critical',
  kind: 'no_data',
  summary: 'no data for 5m',
  started_at: local(2026, 10, 17, 10, 0),
  resolved_at: local(2026, 10, 17, 10, 30),
  resolve_reason: 'condition_ended',
  acknowledged_at: null,
  acknowledged_by: null,
  ack_comment: null,
  closed_by: null,
  close_comment: null,
};

export const criticalState: HostState = {
  severity: 'critical',
  acknowledged: false,
  kind: 'compare',
  trigger: 'Disk almost full',
  instance: 'mount=/',
  summary: 'disk.used_percent > 90: value 94.2',
  open_problems: 1,
};

export const warningAcknowledgedState: HostState = {
  severity: 'warning',
  acknowledged: true,
  kind: 'compare',
  trigger: 'Memory high',
  instance: '',
  summary: 'memory.used_percent >= 80: value 85.1',
  open_problems: 1,
};

export const noDataState: HostState = {
  severity: 'critical',
  acknowledged: false,
  kind: 'no_data',
  trigger: 'Agent silent',
  instance: '',
  summary: 'no data for 5m',
  open_problems: 1,
};
