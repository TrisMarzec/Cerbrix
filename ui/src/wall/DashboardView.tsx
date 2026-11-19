import type { DashboardDetail, Panel, SyncState } from '../api/types';
import { PanelGrid } from './PanelGrid';
import { GraphPanel } from './panels/GraphPanel';
import { HostStatesPanel } from './panels/HostStatesPanel';
import { ProblemsPanel } from './panels/ProblemsPanel';
import { ValuePanel } from './panels/ValuePanel';
import { WallFrame, type WallAlarm } from './WallFrame';

interface Props {
  /** Undefined before the first answer: the frame shows `title` and an empty grid area. */
  dashboard: DashboardDetail | undefined;
  /** The title while there is no dashboard answer yet. */
  title?: string;
  sync?: SyncState;
  now: Date;
  alarm?: WallAlarm;
  /** The ids of the alarms that caused a switch to this dashboard; every problems panel marks them. */
  causes: string[];
  lostSince?: Date;
  /** With `lostSince`: the server answers, but with an error. */
  serverError?: boolean;
  back?: string;
}

/** The content of one panel by its type; a type this page does not know yet keeps its cell. */
function PanelContent({ panel, now, causes }: { panel: Panel; now: Date; causes: string[] }) {
  switch (panel.type) {
    case 'problems':
      return <ProblemsPanel panel={panel} now={now} causes={causes} />;
    case 'host-states':
      return <HostStatesPanel panel={panel} />;
    case 'graph':
      return <GraphPanel panel={panel} now={now} />;
    case 'value':
      return <ValuePanel panel={panel} now={now} />;
    default:
      // A newer server may have panel types this page does not know: they must not break it.
      return (
        <>
          <div class="wall-panel-label">Unknown panel</div>
          <div class="wall-panel-body" />
        </>
      );
  }
}

/** The panels of one dashboard on the 12 by 6 grid, without a frame: what a dashboard change fades. */
export function DashboardGrid({ dashboard, now, causes }: { dashboard: DashboardDetail; now: Date; causes: string[] }) {
  if (dashboard.panels.length === 0) return <p class="wall-no-panels">This dashboard has no panels.</p>;
  return (
    <PanelGrid panels={dashboard.panels} render={(panel) => <PanelContent panel={panel} now={now} causes={causes} />} />
  );
}

/**
 * One dashboard on the wall: the frame with its name, and its panels on the 12 by 6 grid. The frame
 * is the same element before and after the first answer (ruling R11).
 */
export function DashboardView({ dashboard, title, sync, now, alarm, causes, lostSince, serverError, back }: Props) {
  return (
    <WallFrame
      title={dashboard?.name ?? title ?? ''}
      sync={sync}
      now={now}
      alarm={alarm}
      lostSince={lostSince}
      serverError={serverError}
      back={back}
    >
      {dashboard !== undefined && <DashboardGrid dashboard={dashboard} now={now} causes={causes} />}
    </WallFrame>
  );
}
