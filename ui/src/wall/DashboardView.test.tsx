import { render, screen } from '@testing-library/preact';
import type uPlot from 'uplot';
import { vi } from 'vitest';

import type { DashboardDetail, Panel } from '../api/types';
import {
  NOW,
  criticalProblem,
  dashboardDetailFixture,
  problemsPanelFixture,
  statusFixture,
} from '../test/fixtures';
import { DashboardView } from './DashboardView';
import { installMeasure, releaseMeasure } from './test/measure';
import type { WallAlarm } from './WallFrame';

const plots: uPlot.Options[] = [];

vi.mock('uplot', () => ({
  default: class {
    constructor(opts: uPlot.Options) {
      plots.push(opts);
    }
    setSize() {}
    setData() {}
    destroy() {}
  },
}));
vi.mock('uplot/dist/uPlot.min.css', () => ({}));

const innerWidth = Object.getOwnPropertyDescriptor(window, 'innerWidth');

beforeEach(() => {
  plots.length = 0;
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1920 });
  // Room for rows, squares, a plot and a value in every panel.
  installMeasure({
    bodyWidth: 1000,
    bodyHeight: 600,
    rowHeight: 100,
    rowWidth: 400,
    summaryHeight: 60,
    emptyWidth: 500,
    emptyHeight: 60,
    lineHeight: 40,
    square: 30,
    gap: 6,
    charWidth: 10,
  });
  // The plot's container and the value samples, which the helper does not measure.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('graph-canvas')) return new DOMRect(0, 0, 1000, 600);
    if (this.classList.contains('wall-value')) return new DOMRect(0, 0, 300, 200);
    return Element.prototype.getBoundingClientRect.call(this);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  releaseMeasure();
  if (innerWidth !== undefined) Object.defineProperty(window, 'innerWidth', innerWidth);
});

interface Props {
  dashboard?: DashboardDetail;
  causes?: string[];
  alarm?: WallAlarm;
  lostSince?: Date;
  back?: string;
}

function show({ dashboard = dashboardDetailFixture({ name: 'Kunde A' }), causes = [], ...rest }: Props = {}) {
  return render(<DashboardView dashboard={dashboard} now={NOW} causes={causes} {...rest} />);
}

function labels(container: Element): string[] {
  return [...container.querySelectorAll('.wall-panel > .wall-panel-label')].map((l) => l.textContent ?? '');
}

describe('DashboardView', () => {
  it('shows the dashboard name in the frame and one panel of each type on the grid', () => {
    const { container } = show();
    expect(screen.getByRole('heading', { level: 1, name: 'Kunde A' })).toBeInTheDocument();
    expect(container.querySelectorAll('.wall-grid > .wall-panel')).toHaveLength(4);
    expect(labels(container)).toEqual(['Problems', 'Hosts', expect.stringContaining('db-01 · cpu.usage'), 'db-01 · load.1m']);
    expect(container.querySelector('.wall-problem:not(.wall-sample)')).toHaveTextContent('Disk almost full');
    expect(container.querySelector('.wall-squares')).not.toBeNull();
    expect(plots).toHaveLength(1);
    expect(container.querySelector('.wall-value:not(.wall-sample)')).toHaveTextContent('0.42');
  });

  it('places each panel at its cells', () => {
    const { container } = show();
    const style = container.querySelector<HTMLElement>('.wall-panel')?.style;
    expect(style?.gridColumn).toBe('1 / span 8');
    expect(style?.gridRow).toBe('1 / span 6');
  });

  it('marks the problems that caused a switch in every problems panel', () => {
    const dashboard = dashboardDetailFixture({
      panels: [problemsPanelFixture({ w: 6 }), problemsPanelFixture({ x: 6, w: 6 })],
    });
    const { container } = show({ dashboard, causes: [criticalProblem.id] });
    expect(container.querySelectorAll('.wall-problem.is-cause:not(.wall-sample)')).toHaveLength(2);
  });

  it('shows a panel of a type it does not know as an unknown panel, and the others as usual', () => {
    const future = { type: 'heatmap', x: 6, y: 0, w: 6, h: 6 } as unknown as Panel;
    const dashboard = dashboardDetailFixture({ panels: [problemsPanelFixture({ w: 6 }), future] });
    const { container } = show({ dashboard });
    expect(labels(container)).toEqual(['Problems', 'Unknown panel']);
  });

  it('passes the sync state, the alarm, the lost connection and the back link to the frame', () => {
    const sync = { ...statusFixture().sync, status: 'out_of_sync' as const };
    render(
      <DashboardView
        dashboard={dashboardDetailFixture()}
        now={NOW}
        causes={[]}
        sync={sync}
        alarm={{ showing: 'E2E', waiting: 0 }}
        lostSince={new Date(NOW.getTime() - 60_000)}
        back="/dashboards"
      />,
    );
    expect(screen.getByText(/Alarm: showing E2E\./)).toBeInTheDocument();
    expect(screen.getByText(/No connection to the server since/)).toBeInTheDocument();
    expect(screen.getByText(/^Out of sync since/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Back to the desk' })).toHaveAttribute('href', '/dashboards');
  });

  it('says that a dashboard without panels has none, in place of the grid', () => {
    const { container } = show({ dashboard: dashboardDetailFixture({ panels: [] }) });
    expect(container.querySelector('.wall-grid')).toBeNull();
    expect(container.querySelector('.wall-content > .wall-no-panels')).toHaveTextContent(
      'This dashboard has no panels.',
    );
  });

  it('says that the server reported an error when told so', () => {
    render(
      <DashboardView
        dashboard={dashboardDetailFixture()}
        now={NOW}
        causes={[]}
        lostSince={new Date(NOW.getTime() - 60_000)}
        serverError
      />,
    );
    expect(screen.getByText(/^The server reported an error since/)).toHaveTextContent(
      'The server reported an error since 14:29',
    );
    expect(screen.queryByText(/No connection/)).not.toBeInTheDocument();
  });
});
