import type {
  DashboardDetail,
  DashboardSummary,
  Display,
  EnrollmentKey,
  GraphPanelData,
  HostDetail,
  HostRow,
  HostState,
  HostStatesPanelData,
  Problem,
  ProblemsPanelData,
  Status,
  SyncHistoryEntry,
  User,
  ValuePanelData,
} from '../api/types';

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

const NO_PROBLEMS = { open: 0, unacknowledged: 0 };

/** An in-sync installation with GitOps on and no open problems; override what a test needs. */
export function statusFixture(overrides: Partial<Status> = {}): Status {
  return {
    version: '0.1.0',
    gitops: true,
    mail: true,
    sync: {
      status: 'in_sync',
      active_commit: '253a2994b13aca387e272ff08db9eb858b436b66',
      last_success_at: new Date(2026, 10, 17, 14, 28, 0).toISOString(),
      last_attempt_at: new Date(2026, 10, 17, 14, 29, 0).toISOString(),
      issues: [],
    },
    hosts: { total: 3, with_problems: 0, without_problems: 3, never_seen: 0 },
    problems: {
      critical: { ...NO_PROBLEMS },
      high: { ...NO_PROBLEMS },
      warning: { ...NO_PROBLEMS },
      info: { ...NO_PROBLEMS },
    },
    ...overrides,
  };
}

const NO_COUNTS = { critical: 0, high: 0, warning: 0, info: 0 };

/** A dashboard with no hosts and no problems; override what a test needs. */
export function dashboardFixture(overrides: Partial<DashboardSummary> = {}): DashboardSummary {
  return {
    id: 'e2e',
    name: 'E2E',
    file: 'dashboards/e2e.json',
    priority: 0,
    host_count: 0,
    hosts_by_state: { critical: 0, high: 0, warning: 0, info: 0, acknowledged: 0, ok: 0 },
    open_problems: { ...NO_COUNTS },
    unacknowledged: { ...NO_COUNTS },
    ...overrides,
  };
}

/** A problems panel over the left two thirds with the critical problem; override what a test needs. */
export function problemsPanelFixture(overrides: Partial<ProblemsPanelData> = {}): ProblemsPanelData {
  return { type: 'problems', x: 0, y: 0, w: 8, h: 6, problems: [criticalProblem], more: 0, ...overrides };
}

/** A host-states panel with one critical host of one; override what a test needs. */
export function hostStatesPanelFixture(overrides: Partial<HostStatesPanelData> = {}): HostStatesPanelData {
  return {
    type: 'host-states',
    x: 8,
    y: 0,
    w: 4,
    h: 3,
    total: 1,
    without_problems: 0,
    hosts: [{ name: 'db-01', state: criticalState }],
    ...overrides,
  };
}

/** A graph panel with two points and one threshold; override what a test needs. */
export function graphPanelFixture(overrides: Partial<GraphPanelData> = {}): GraphPanelData {
  return {
    type: 'graph',
    x: 8,
    y: 3,
    w: 4,
    h: 2,
    host: 'db-01',
    item: 'cpu.usage',
    params: {},
    range: '1h',
    step_secs: 15,
    points: [
      [NOW.getTime() - 15_000, 28.2028656006],
      [NOW.getTime(), 27.432800292968],
    ],
    thresholds: [{ trigger: 'CPU high', severity: 'high', op: '>', value: 90 }],
    ...overrides,
  };
}

/** A value panel with a value measured 30 seconds before `NOW`; override what a test needs. */
export function valuePanelFixture(overrides: Partial<ValuePanelData> = {}): ValuePanelData {
  return {
    type: 'value',
    x: 8,
    y: 5,
    w: 4,
    h: 1,
    host: 'db-01',
    item: 'load.1m',
    params: {},
    value: 0.42,
    at: new Date(NOW.getTime() - 30_000).toISOString(),
    ...overrides,
  };
}

/** A dashboard with one host and one panel of each type (docs/api.md); override what a test needs. */
export function dashboardDetailFixture(overrides: Partial<DashboardDetail> = {}): DashboardDetail {
  return {
    id: 'e2e',
    name: 'E2E',
    file: 'dashboards/e2e.json',
    priority: 0,
    hosts: [{ name: 'db-01', state: criticalState }],
    panels: [problemsPanelFixture(), hostStatesPanelFixture(), graphPanelFixture(), valuePanelFixture()],
    metrics_available: true,
    ...overrides,
  };
}

/** A display that shows one dashboard, does not switch and follows its setting. */
export function displayFixture(overrides: Partial<Display> = {}): Display {
  return {
    id: 'e2e-wall',
    name: 'E2E wall',
    file: 'displays/e2e-wall.json',
    show: { dashboard: 'e2e' },
    switch_on_alarm: null,
    dashboards: null,
    token_created_at: null,
    target: null,
    causes: [],
    waiting: [],
    worst: null,
    ...overrides,
  };
}

/** A healthy host that reported 10 seconds before `NOW`; override what a test needs. */
export function hostRowFixture(overrides: Partial<HostRow> = {}): HostRow {
  return {
    name: 'web-01',
    address: '10.20.1.21',
    groups: ['web'],
    state: null,
    last_seen_at: new Date(NOW.getTime() - 10_000).toISOString(),
    managed_by_git: true,
    cpu: 41.2,
    memory: 58.04,
    ...overrides,
  };
}

/** A host that has reported, with one item, one trigger and no open problems; override what a test needs. */
export function hostDetailFixture(overrides: Partial<HostDetail> = {}): HostDetail {
  return {
    name: 'web-01',
    address: '10.20.1.21',
    groups: ['web'],
    templates: ['linux-base'],
    source_file: 'hosts/web-01.json',
    managed_by_git: true,
    enrolled_at: new Date(2026, 10, 16, 21, 52).toISOString(),
    last_seen_at: new Date(NOW.getTime() - 10_000).toISOString(),
    agent_version: '0.1.0',
    dashboards: [],
    state: null,
    items: [{ key: 'cpu.usage', params: null, interval_secs: 5, origin: 'host' }],
    triggers: [
      {
        name: 'CPU high',
        expression: 'cpu.usage > 90 for 5m',
        severity: 'high',
        origin: 'template:linux-base',
        firing: false,
      },
    ],
    open_problems: [],
    ...overrides,
  };
}

/** Fresh arrays every time, so fixtures never share them. */
function noNames() {
  return { added: [], changed: [], removed: [] };
}

/** An applied revision that changed nothing; override `changes` or the rest. */
export function syncEntryFixture(overrides: Partial<SyncHistoryEntry> = {}): SyncHistoryEntry {
  return {
    commit: '253a2994b13aca387e272ff08db9eb858b436b66',
    applied_at: new Date(2026, 10, 17, 13, 0, 0).toISOString(),
    changes: {
      hosts: noNames(),
      templates: noNames(),
      notifications: noNames(),
      dashboards: noNames(),
      displays: noNames(),
    },
    ...overrides,
  };
}

/** An active enrolment key; override what a test needs. */
export function keyFixture(overrides: Partial<EnrollmentKey> = {}): EnrollmentKey {
  return {
    id: 'd5293479-d2de-4693-9da5-99c726925d97',
    description: 'web-02',
    created_at: new Date(2026, 10, 17, 12, 0, 0).toISOString(),
    expires_at: new Date(2026, 10, 18, 12, 0, 0).toISOString(),
    revoked_at: null,
    state: 'active',
    ...overrides,
  };
}

/** An enabled viewer whose password was set at noon; override what a test needs. */
export function userFixture(overrides: Partial<User> = {}): User {
  return {
    id: 'd0e7fedb-ed62-4568-a7b7-6a28a0f0f3b5',
    name: 'bob',
    role: 'viewer',
    disabled: false,
    created_at: local(2026, 10, 16, 21, 54),
    password_changed_at: local(2026, 10, 17, 12, 0),
    ...overrides,
  };
}
